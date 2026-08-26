//! P1 release identity and queryable capability output.

use crate::config_load::LoadedConfig;
use open_compute_core::{
    CacheConfig, CapabilityStatus, D1Config, DurableObjectsConfig, ErrorCode, HardeningConfig,
    KvConfig, PlatformCapabilitiesV1, PlatformError, PlatformReleaseIdentityV1,
    PlatformReleaseMetadataV1, ProductCapabilityV1, R2Config, ReleaseMigrationV1,
    RuntimeCapabilityV1, SchedulerConfig, WorkersConfig,
};
use open_compute_runtime::{load_runtime_lock, runtime_assets_sha256};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, KV_SCHEMA_VERSION, current_scheduler_schema_version, migrations,
};
use open_compute_workers::{
    COMPATIBILITY_DATE_MAX, COMPATIBILITY_DATE_MIN, COMPATIBILITY_FLAGS_ALLOWED,
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
    cache: &'a CacheConfig,
}

/// Build the complete production capability registry from formal files and constants.
pub fn platform_capabilities(
    loaded: &LoadedConfig,
) -> Result<PlatformCapabilitiesV1, PlatformError> {
    let (runtime_lock, lock_bytes) = load_runtime_lock(&loaded.config.runtime.lock_file)?;
    let lock_sha256 = hex::encode(Sha256::digest(&lock_bytes));
    let assets_sha256 = runtime_assets_sha256(&loaded.config.runtime.assets_dir)?;
    let compatibility_policy_sha256 = compatibility_policy_sha256();
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
        facade_capability_version: FACADE_CAPABILITY_VERSION,
        control_schema_version: u32::try_from(migrations::current_schema_version())
            .map_err(|_| capability_invalid())?,
        scheduler_schema_version: u32::try_from(current_scheduler_schema_version())
            .map_err(|_| capability_invalid())?,
        kv_schema_version_min: KV_SCHEMA_VERSION,
        kv_schema_version_max: KV_SCHEMA_VERSION,
        d1_schema_version_min: D1_DATABASE_SCHEMA_VERSION,
        d1_schema_version_max: D1_DATABASE_SCHEMA_VERSION,
        snapshot_format_version: SNAPSHOT_FORMAT_VERSION,
        compatibility_policy_sha256,
    };
    let products = product_registry();
    let limits = limit_registry(loaded);
    let capabilities = PlatformCapabilitiesV1 {
        schema_version: 1,
        release,
        runtime: RuntimeCapabilityV1 {
            compatibility_date_min: COMPATIBILITY_DATE_MIN.to_owned(),
            compatibility_date_max: COMPATIBILITY_DATE_MAX.to_owned(),
            allowed_flags: COMPATIBILITY_FLAGS_ALLOWED
                .iter()
                .map(ToString::to_string)
                .collect(),
            denied_flags: vec![
                "nodejs_compat_populate_process_env".to_owned(),
                "unsafe_module".to_owned(),
            ],
            workerd_lock_sha256: lock_sha256,
        },
        products,
        limits,
    };
    if !capabilities.validate() {
        return Err(capability_invalid());
    }
    Ok(capabilities)
}

/// Build the package and upgrade metadata from the same production registries.
pub fn platform_release_metadata(
    loaded: &LoadedConfig,
) -> Result<PlatformReleaseMetadataV1, PlatformError> {
    let release = platform_capabilities(loaded)?.release;
    let migrations = migrations::migration_registry()
        .into_iter()
        .map(|(version, name, digest)| {
            Ok(ReleaseMigrationV1 {
                version: u32::try_from(version).map_err(|_| capability_invalid())?,
                name: name.to_owned(),
                sha256: hex::encode(digest),
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    let version = release.platform_version.clone();
    let metadata = PlatformReleaseMetadataV1 {
        schema_version: 1,
        release,
        upgrade_from_control_schema_min: 7,
        upgrade_from_platform_versions: vec![version.clone()],
        restore_compatible_platform_versions: vec![version],
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
        ]),
        migrations,
        readable_object_formats: BTreeMap::from([
            ("artifacts".to_owned(), vec![1]),
            ("kv_backups".to_owned(), vec![1]),
            ("d1_backups".to_owned(), vec![1]),
            ("r2".to_owned(), vec![1]),
            ("snapshots".to_owned(), vec![1]),
        ]),
        workerd_local_disk_gate_result: "p0.7-stock-workerd".to_owned(),
        conformance_result: "p1.0-capabilities-v1".to_owned(),
        websocket_hibernation_result: "no-go:p1.8-unsupported".to_owned(),
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
        sqlite_busy_timeout_ms: config.storage.sqlite_busy_timeout_ms,
        free_space_soft_bytes: config.storage.free_space_soft_bytes,
        free_space_hard_bytes: config.storage.free_space_hard_bytes,
        hardening: &config.hardening,
        workers: &config.workers,
        kv: &config.kv,
        r2: &config.r2,
        d1: &config.d1,
        durable_objects: &config.durable_objects,
        scheduler: &config.scheduler,
        cache: &config.cache,
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
            writeln!(out, "{name} {:?}", capability.status).map_err(|_| capability_invalid())?;
        }
    }
    Ok(())
}

fn product_registry() -> BTreeMap<String, ProductCapabilityV1> {
    let mut products = BTreeMap::new();
    products.insert(
        "workers".to_owned(),
        supported(
            &["fetch", "rpc", "streams", "websocket", "outbound_fetch"],
            &[],
        ),
    );
    products.insert(
        "kv".to_owned(),
        supported(
            &["get", "getWithMetadata", "put", "delete", "list", "getBulk"],
            &["OC-KV-001"],
        ),
    );
    products.insert(
        "r2".to_owned(),
        supported(
            &["head", "get", "put", "delete", "list", "deleteMany"],
            &["OC-R2-001"],
        ),
    );
    products.insert(
        "d1".to_owned(),
        supported(
            &[
                "prepare",
                "batch",
                "exec",
                "withSession",
                "run",
                "all",
                "first",
                "raw",
            ],
            &["OC-D1-001"],
        ),
    );
    let mut durable_objects = supported(
        &[
            "idFromName",
            "newUniqueId",
            "idFromString",
            "get",
            "getByName",
            "fetch",
            "rpc",
        ],
        &["OC-DO-001", "OC-WS-001"],
    );
    durable_objects.basic_websocket = Some(CapabilityStatus::Supported);
    durable_objects.hibernatable_websocket = Some(CapabilityStatus::Unsupported);
    products.insert("durable_objects".to_owned(), durable_objects);
    products.insert(
        "alarms".to_owned(),
        supported(&["getAlarm", "setAlarm", "deleteAlarm", "alarm"], &[]),
    );
    for name in ["queues", "cron", "workflows", "websocket_hibernation"] {
        products.insert(name.to_owned(), unsupported());
    }
    products
}

fn supported(methods: &[&str], deviations: &[&str]) -> ProductCapabilityV1 {
    ProductCapabilityV1 {
        status: CapabilityStatus::Supported,
        capability_version: Some(FACADE_CAPABILITY_VERSION),
        methods: methods.iter().map(ToString::to_string).collect(),
        deviations: deviations.iter().map(ToString::to_string).collect(),
        basic_websocket: None,
        hibernatable_websocket: None,
    }
}

fn unsupported() -> ProductCapabilityV1 {
    ProductCapabilityV1 {
        status: CapabilityStatus::Unsupported,
        capability_version: None,
        methods: Vec::new(),
        deviations: Vec::new(),
        basic_websocket: None,
        hibernatable_websocket: None,
    }
}

fn limit_registry(loaded: &LoadedConfig) -> BTreeMap<String, u64> {
    let config = &loaded.config;
    BTreeMap::from([
        (
            "workers.max_bundle_bytes".to_owned(),
            config.workers.max_bundle_bytes,
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
            "hardening.max_workers_per_account".to_owned(),
            u64::from(config.hardening.max_workers_per_account),
        ),
        (
            "hardening.max_routes_per_account".to_owned(),
            u64::from(config.hardening.max_routes_per_account),
        ),
        (
            "hardening.max_deployments_per_worker".to_owned(),
            u64::from(config.hardening.max_deployments_per_worker),
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
            "hardening.snapshot_stale_after_ms".to_owned(),
            config.hardening.snapshot_stale_after_ms,
        ),
    ])
}

fn compatibility_policy_sha256() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/compatibility-policy/v1\0");
    for value in std::iter::once(COMPATIBILITY_DATE_MIN)
        .chain(std::iter::once(COMPATIBILITY_DATE_MAX))
        .chain(COMPATIBILITY_FLAGS_ALLOWED.iter().copied())
    {
        hasher.update(value.len().to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn capability_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "platform capability registry is invalid",
    )
}
