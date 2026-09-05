//! P1 release identity and queryable capability output.

use crate::config_load::LoadedConfig;
use crate::embedded_dashboard::embedded_dashboard_assets_sha256;
use open_compute_core::config::ObservabilityConfig;
use open_compute_core::{
    CacheConfig, CapabilityInventoryV1, D1Config, DurableObjectsConfig, ErrorCode, HardeningConfig,
    KvConfig, ManagementApiCapabilitiesV1, PlatformCapabilitiesV1, PlatformConfig, PlatformError,
    PlatformReleaseIdentityV1, PlatformReleaseMetadataV1, ProductCapabilityV1, R2Config,
    ReleaseSchemaDefinitionV1, RuntimeCapabilityV1, SchedulerConfig, TypeSourceIdentityV1,
    WorkersConfig, WorkersObservabilityCapabilitiesV1, WranglerCapabilitiesV1,
};
use open_compute_runtime::{embedded_runtime_assets_sha256, embedded_runtime_lock};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, KV_SCHEMA_VERSION, QUEUE_MAX_BATCH_BYTES, QUEUE_MAX_BATCH_MESSAGES,
    QUEUE_MAX_DELAY_SECONDS, QUEUE_MAX_MESSAGE_BYTES, ai_search::AI_SEARCH_SCHEMA_VERSION,
    current_scheduler_schema_version, migrations, vectorize::VECTORIZE_SCHEMA_VERSION,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;

const FACADE_CAPABILITY_VERSION: u32 = 1;
const SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Serialize)]
struct SnapshotPolicyV1<'a> {
    schema_version: u32,
    sqlite_busy_timeout_ms: u64,
    free_space_soft_bytes: u64,
    free_space_hard_bytes: u64,
    hardening: &'a HardeningConfig,
    workers: &'a WorkersConfig,
    kv: &'a KvConfig,
    r2: &'a R2Config,
    d1: &'a D1Config,
    durable_objects: &'a DurableObjectsConfig,
    scheduler: &'a SchedulerConfig,
    workflows: &'a open_compute_core::WorkflowsConfig,
    cache: &'a CacheConfig,
    response_cache: &'a open_compute_core::ResponseCacheConfig,
    images: &'a open_compute_core::ImagesConfig,
    document_parser: &'a open_compute_core::DocumentParserConfig,
    observability: &'a ObservabilityConfig,
}

type ProductRegistry = (
    TypeSourceIdentityV1,
    BTreeMap<String, ProductCapabilityV1>,
    ManagementApiCapabilitiesV1,
    WorkersObservabilityCapabilitiesV1,
    WranglerCapabilitiesV1,
);

/// Build the complete production capability registry from embedded release inputs.
pub fn platform_capabilities(
    config: &PlatformConfig,
) -> Result<PlatformCapabilitiesV1, PlatformError> {
    let (runtime_lock, lock_bytes) = embedded_runtime_lock()?;
    let lock_sha256 = hex::encode(Sha256::digest(lock_bytes));
    let assets_sha256 = embedded_runtime_assets_sha256().to_owned();
    let release = PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: env!("CARGO_PKG_VERSION").to_owned(),
        git_revision: option_env!("OPEN_COMPUTE_GIT_REVISION")
            .unwrap_or("unknown")
            .to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: runtime_lock.expected_version_output.clone(),
        workerd_lock_sha256: lock_sha256.clone(),
        runtime_assets_sha256: assets_sha256,
        dashboard_assets_sha256: embedded_dashboard_assets_sha256().to_owned(),
        facade_capability_version: FACADE_CAPABILITY_VERSION,
        control_schema_version: u32::try_from(migrations::current_schema_version())
            .map_err(|_| capability_invalid())?,
        scheduler_schema_version: u32::try_from(current_scheduler_schema_version())
            .map_err(|_| capability_invalid())?,
        kv_schema_version: KV_SCHEMA_VERSION,
        d1_schema_version: D1_DATABASE_SCHEMA_VERSION,
        vectorize_schema_version: VECTORIZE_SCHEMA_VERSION,
        ai_search_schema_version: AI_SEARCH_SCHEMA_VERSION,
        snapshot_format_version: SNAPSHOT_FORMAT_VERSION,
    };
    let (type_source, products, management_api, workers_observability, wrangler) =
        product_registry()?;
    let limits = limit_registry(config);
    let capabilities = PlatformCapabilitiesV1 {
        schema_version: 1,
        release,
        runtime: RuntimeCapabilityV1 {
            effective_compatibility_date: runtime_lock.effective_compatibility_date.clone(),
            workerd_lock_sha256: lock_sha256,
            workers_types_version: type_source.workers_types_version,
            workers_types_git_head: type_source.git_head,
            workers_types_package_sha256: type_source.package_sha256,
            workers_types_index_sha256: type_source.index_sha256,
            workers_types_ast_sha256: type_source.ast_sha256,
        },
        products,
        management_api,
        workers_observability,
        wrangler,
        limits,
    };
    if !capabilities.validate() {
        return Err(capability_invalid());
    }
    Ok(capabilities)
}

/// Build the current release metadata from the same production registries.
pub fn platform_release_metadata(
    loaded: &LoadedConfig,
) -> Result<PlatformReleaseMetadataV1, PlatformError> {
    let release = platform_capabilities(&loaded.config)?.release;
    let schema_definitions = migrations::migration_registry()
        .into_iter()
        .map(|(version, name, digest)| {
            Ok(ReleaseSchemaDefinitionV1 {
                version: u32::try_from(version).map_err(|_| capability_invalid())?,
                name: name.to_owned(),
                sha256: hex::encode(digest),
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    let metadata = PlatformReleaseMetadataV1 {
        schema_version: 1,
        release,
        target_schemas: BTreeMap::from([
            (
                "control".to_owned(),
                u32::try_from(migrations::current_schema_version())
                    .map_err(|_| capability_invalid())?,
            ),
            (
                "scheduler".to_owned(),
                u32::try_from(current_scheduler_schema_version())
                    .map_err(|_| capability_invalid())?,
            ),
            ("kv".to_owned(), KV_SCHEMA_VERSION),
            ("d1".to_owned(), D1_DATABASE_SCHEMA_VERSION),
            ("vectorize".to_owned(), VECTORIZE_SCHEMA_VERSION),
            ("ai_search".to_owned(), AI_SEARCH_SCHEMA_VERSION),
        ]),
        schema_definitions,
        object_formats: BTreeMap::from([
            ("ai_search_objects".to_owned(), 1),
            ("artifacts".to_owned(), 1),
            ("kv_backups".to_owned(), 1),
            ("d1_backups".to_owned(), 1),
            ("r2".to_owned(), 1),
            ("snapshots".to_owned(), 1),
        ]),
        workerd_local_disk_gate_result: "p0.7-stock-workerd".to_owned(),
        conformance_result: "workflow-current".to_owned(),
        websocket_hibernation_result: "p0.7-stock-workerd".to_owned(),
    };
    if !metadata.validate() {
        return Err(capability_invalid());
    }
    Ok(metadata)
}

/// Hash the redacted storage and product policy that a fresh restore must preserve.
pub fn platform_config_policy_sha256(loaded: &LoadedConfig) -> Result<String, PlatformError> {
    let config = &loaded.config;
    let bytes = serde_json::to_vec(&SnapshotPolicyV1 {
        schema_version: 1,
        sqlite_busy_timeout_ms: config.data.sqlite_busy_timeout_ms,
        free_space_soft_bytes: config.data.free_space_soft_bytes,
        free_space_hard_bytes: config.data.free_space_hard_bytes,
        hardening: &config.hardening,
        workers: &config.workers,
        kv: &config.kv,
        r2: &config.r2,
        d1: &config.d1,
        durable_objects: &config.durable_objects,
        scheduler: &config.scheduler,
        workflows: &config.workflows,
        cache: &config.cache,
        response_cache: &config.response_cache,
        images: &config.images,
        document_parser: &config.document_parser,
        observability: &config.observability,
    })
    .map_err(|_| capability_invalid())?;
    let mut digest = Sha256::new();
    digest.update(b"open-compute/snapshot-config-policy/v1\0");
    digest.update(bytes);
    Ok(hex::encode(digest.finalize()))
}

/// Write deterministic human or versioned JSON capability output.
pub fn write_capabilities(
    capabilities: &PlatformCapabilitiesV1,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    if json {
        serde_json::to_writer(&mut *out, capabilities).map_err(|_| capability_invalid())?;
        writeln!(out).map_err(|_| capability_invalid())?;
    } else {
        writeln!(out, "CAPABILITIES V{}", capabilities.schema_version)
            .map_err(|_| capability_invalid())?;
        writeln!(
            out,
            "release={} workerd={}",
            capabilities.release.platform_version, capabilities.release.workerd_version
        )
        .map_err(|_| capability_invalid())?;
        for (name, capability) in &capabilities.products {
            writeln!(
                out,
                "{name} {:?} members={}",
                capability.status,
                capability.members.len()
            )
            .map_err(|_| capability_invalid())?;
        }
    }
    Ok(())
}

fn product_registry() -> Result<ProductRegistry, PlatformError> {
    let inventory: CapabilityInventoryV1 = serde_json::from_slice(include_bytes!(
        "../../../share/cloudflare-capabilities.json"
    ))
    .map_err(|_| capability_invalid())?;
    if !inventory.validate() {
        return Err(capability_invalid());
    }
    Ok((
        inventory.source,
        inventory.products,
        inventory.management_api,
        inventory.workers_observability,
        inventory.wrangler,
    ))
}

fn limit_registry(config: &PlatformConfig) -> BTreeMap<String, u64> {
    let alarm = config
        .scheduler
        .pool(open_compute_core::SchedulerKind::Alarm);
    let queue = config
        .scheduler
        .pool(open_compute_core::SchedulerKind::Queue);
    let cron = config
        .scheduler
        .pool(open_compute_core::SchedulerKind::Cron);
    BTreeMap::from([
        (
            "workflows.max_json_bytes".to_owned(),
            open_compute_core::workflow::WORKFLOW_VALUE_MAX_BYTES as u64,
        ),
        (
            "workflows.max_steps".to_owned(),
            u64::from(config.workflows.max_steps),
        ),
        (
            "workflows.max_state_bytes".to_owned(),
            config.workflows.max_state_bytes,
        ),
        (
            "workflows.max_account_state_bytes".to_owned(),
            config.workflows.max_account_state_bytes,
        ),
        (
            "workflows.max_instances_per_account".to_owned(),
            u64::from(config.workflows.max_instances_per_account),
        ),
        (
            "workflows.max_instances_per_definition".to_owned(),
            u64::from(config.workflows.max_instances_per_definition),
        ),
        (
            "workflows.max_active_per_account".to_owned(),
            u64::from(config.workflows.max_active_per_account),
        ),
        ("workflows.lease_ms".to_owned(), config.workflows.lease_ms),
        (
            "workflows.heartbeat_ms".to_owned(),
            config.workflows.heartbeat_ms,
        ),
        (
            "workflows.dispatch_timeout_ms".to_owned(),
            config.workflows.dispatch_timeout_ms,
        ),
        (
            "workflows.max_parallel_steps".to_owned(),
            u64::from(config.workflows.max_parallel_steps),
        ),
        (
            "workflows.max_buffered_events".to_owned(),
            u64::from(config.workflows.max_buffered_events),
        ),
        (
            "workflows.max_event_bytes".to_owned(),
            config.workflows.max_event_bytes,
        ),
        (
            "workflows.default_success_retention_ms".to_owned(),
            config.workflows.default_retention.success_retention_ms,
        ),
        (
            "workflows.default_error_retention_ms".to_owned(),
            config.workflows.default_retention.error_retention_ms,
        ),
        (
            "workflows.max_attempt_ms".to_owned(),
            config
                .workflows
                .dispatch_timeout_ms
                .saturating_sub(open_compute_core::workflow::WORKFLOW_DRAIN_MARGIN_MS)
                .min(open_compute_core::workflow::WORKFLOW_MAX_ATTEMPT_MS),
        ),
        (
            "workflows.max_retry_delay_ms".to_owned(),
            open_compute_core::workflow::WORKFLOW_MAX_RETRY_DELAY_MS,
        ),
        (
            "workflows.max_duration_ms".to_owned(),
            open_compute_core::workflow::WORKFLOW_MAX_DURATION_MS,
        ),
        (
            "scheduler.pools.workflow.max_in_flight".to_owned(),
            u64::from(
                config
                    .scheduler
                    .pool(open_compute_core::SchedulerKind::Workflow)
                    .max_in_flight,
            ),
        ),
        (
            "workers.max_bundle_bytes".to_owned(),
            config.workers.max_bundle_bytes,
        ),
        (
            "observability.retention_ms".to_owned(),
            config.observability.retention_ms,
        ),
        (
            "observability.max_database_bytes".to_owned(),
            config.observability.max_database_bytes,
        ),
        (
            "observability.max_invocation_log_bytes".to_owned(),
            config.observability.max_invocation_log_bytes,
        ),
        (
            "observability.max_tail_sessions_per_script".to_owned(),
            u64::from(config.observability.max_tail_sessions_per_script),
        ),
        (
            "observability.query_max_events".to_owned(),
            u64::from(config.observability.query_max_events),
        ),
        (
            "observability.query_max_timeframe_ms".to_owned(),
            config.observability.query_max_timeframe_ms,
        ),
        (
            "kv.namespace_quota_bytes".to_owned(),
            config.kv.namespace_quota_bytes,
        ),
        ("r2.max_object_bytes".to_owned(), config.r2.max_object_bytes),
        (
            "r2.max_staging_bytes".to_owned(),
            config.r2.max_staging_bytes,
        ),
        (
            "d1.database_quota_bytes".to_owned(),
            config.d1.database_quota_bytes,
        ),
        (
            "durable_objects.max_in_flight_dispatches".to_owned(),
            u64::from(config.durable_objects.max_in_flight_dispatches),
        ),
        (
            "scheduler.max_in_flight".to_owned(),
            u64::from(config.scheduler.max_in_flight),
        ),
        (
            "scheduler.pools.alarm.max_in_flight".to_owned(),
            u64::from(alarm.max_in_flight),
        ),
        (
            "scheduler.pools.alarm.claim_batch".to_owned(),
            u64::from(alarm.claim_batch),
        ),
        (
            "scheduler.pools.alarm.weight".to_owned(),
            u64::from(alarm.weight),
        ),
        (
            "scheduler.pools.queue.max_in_flight".to_owned(),
            u64::from(queue.max_in_flight),
        ),
        (
            "scheduler.pools.queue.claim_batch".to_owned(),
            u64::from(queue.claim_batch),
        ),
        (
            "scheduler.pools.queue.weight".to_owned(),
            u64::from(queue.weight),
        ),
        (
            "scheduler.pools.cron.max_in_flight".to_owned(),
            u64::from(cron.max_in_flight),
        ),
        (
            "scheduler.pools.cron.claim_batch".to_owned(),
            u64::from(cron.claim_batch),
        ),
        (
            "scheduler.pools.cron.weight".to_owned(),
            u64::from(cron.weight),
        ),
        (
            "scheduler.cron_misfire_grace_ms".to_owned(),
            config.scheduler.cron_misfire_grace_ms,
        ),
        (
            "scheduler.cron_max_retries".to_owned(),
            u64::from(config.scheduler.cron_max_retries),
        ),
        (
            "scheduler.cron_history_limit".to_owned(),
            u64::from(config.scheduler.cron_history_limit),
        ),
        (
            "queues.max_message_bytes".to_owned(),
            QUEUE_MAX_MESSAGE_BYTES,
        ),
        (
            "queues.max_batch_messages".to_owned(),
            u64::from(QUEUE_MAX_BATCH_MESSAGES),
        ),
        ("queues.max_batch_bytes".to_owned(), QUEUE_MAX_BATCH_BYTES),
        (
            "queues.max_delay_seconds".to_owned(),
            u64::from(QUEUE_MAX_DELAY_SECONDS),
        ),
        (
            "queues.default_max_backlog_bytes".to_owned(),
            config.queues.default_max_backlog_bytes,
        ),
        (
            "queues.max_in_flight_requests".to_owned(),
            u64::from(config.queues.max_in_flight_requests),
        ),
        (
            "queues.max_in_flight_requests_per_binding".to_owned(),
            u64::from(config.queues.max_in_flight_requests_per_binding),
        ),
        (
            "queues.max_consumer_concurrency".to_owned(),
            u64::from(config.queues.max_consumer_concurrency),
        ),
        (
            "hardening.max_workers_per_account".to_owned(),
            u64::from(config.hardening.max_workers_per_account),
        ),
        (
            "hardening.max_routes_per_account".to_owned(),
            u64::from(config.hardening.max_routes_per_account),
        ),
        (
            "hardening.max_versions_per_worker".to_owned(),
            u64::from(config.hardening.max_versions_per_worker),
        ),
        (
            "hardening.max_resources_per_kind_per_account".to_owned(),
            u64::from(config.hardening.max_resources_per_kind_per_account),
        ),
        (
            "hardening.max_snapshot_total_bytes".to_owned(),
            config.hardening.max_snapshot_total_bytes,
        ),
        (
            "response_cache.max_object_bytes".to_owned(),
            config.response_cache.max_object_bytes,
        ),
        (
            "response_cache.max_bytes_per_worker".to_owned(),
            config.response_cache.max_bytes_per_worker,
        ),
        (
            "response_cache.max_header_bytes".to_owned(),
            u64::from(config.response_cache.max_header_bytes),
        ),
        (
            "response_cache.max_variants_per_key".to_owned(),
            u64::from(config.response_cache.max_variants_per_key),
        ),
        (
            "response_cache.max_tags_per_entry".to_owned(),
            u64::from(config.response_cache.max_tags_per_entry),
        ),
        (
            "response_cache.max_cache_name_bytes".to_owned(),
            u64::from(config.response_cache.max_cache_name_bytes),
        ),
        (
            "response_cache.max_url_bytes".to_owned(),
            u64::from(config.response_cache.max_url_bytes),
        ),
        (
            "response_cache.max_connections".to_owned(),
            u64::from(config.response_cache.max_connections),
        ),
        (
            "response_cache.busy_timeout_ms".to_owned(),
            config.response_cache.busy_timeout_ms,
        ),
        (
            "response_cache.request_timeout_ms".to_owned(),
            config.response_cache.request_timeout_ms,
        ),
        (
            "response_cache.refresh_lease_ms".to_owned(),
            config.response_cache.refresh_lease_ms,
        ),
        (
            "response_cache.max_ttl_seconds".to_owned(),
            config.response_cache.max_ttl_seconds,
        ),
        (
            "response_cache.fail_open".to_owned(),
            u64::from(config.response_cache.fail_open),
        ),
        (
            "images.max_input_bytes".to_owned(),
            config.images.max_input_bytes,
        ),
        (
            "images.max_output_bytes".to_owned(),
            config.images.max_output_bytes,
        ),
        ("images.max_pixels".to_owned(), config.images.max_pixels),
        (
            "images.max_dimension".to_owned(),
            u64::from(config.images.max_dimension),
        ),
        (
            "images.max_operations".to_owned(),
            u64::from(config.images.max_operations),
        ),
        (
            "images.max_overlays".to_owned(),
            u64::from(config.images.max_overlays),
        ),
        (
            "images.max_frames".to_owned(),
            u64::from(config.images.max_frames),
        ),
        (
            "images.max_sessions".to_owned(),
            u64::from(config.images.max_sessions),
        ),
        (
            "images.max_temp_bytes".to_owned(),
            config.images.max_temp_bytes,
        ),
        (
            "images.session_ttl_ms".to_owned(),
            config.images.session_ttl_ms,
        ),
        (
            "images.max_concurrency".to_owned(),
            u64::from(config.images.max_concurrency),
        ),
        (
            "images.max_concurrency_per_account".to_owned(),
            u64::from(config.images.max_concurrency_per_account),
        ),
        (
            "images.request_timeout_ms".to_owned(),
            config.images.request_timeout_ms,
        ),
        (
            "document_parser.max_input_bytes".to_owned(),
            config.document_parser.max_input_bytes,
        ),
        (
            "document_parser.max_batch_bytes".to_owned(),
            config.document_parser.max_batch_bytes,
        ),
        (
            "document_parser.max_batch_files".to_owned(),
            u64::from(config.document_parser.max_batch_files),
        ),
        (
            "document_parser.max_output_bytes".to_owned(),
            config.document_parser.max_output_bytes,
        ),
        (
            "document_parser.max_concurrency".to_owned(),
            u64::from(config.document_parser.max_concurrency),
        ),
        (
            "document_parser.max_concurrency_per_account".to_owned(),
            u64::from(config.document_parser.max_concurrency_per_account),
        ),
        (
            "document_parser.max_concurrency_per_version".to_owned(),
            u64::from(config.document_parser.max_concurrency_per_version),
        ),
        (
            "document_parser.request_timeout_ms".to_owned(),
            config.document_parser.request_timeout_ms,
        ),
        (
            "document_parser.max_address_space_bytes".to_owned(),
            config.document_parser.max_address_space_bytes,
        ),
        (
            "document_parser.max_cpu_seconds".to_owned(),
            config.document_parser.max_cpu_seconds,
        ),
        (
            "document_parser.max_stderr_bytes".to_owned(),
            config.document_parser.max_stderr_bytes,
        ),
        (
            "hardening.snapshot_stale_after_ms".to_owned(),
            config.hardening.snapshot_stale_after_ms,
        ),
    ])
}

fn capability_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "platform capability registry is invalid",
    )
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
