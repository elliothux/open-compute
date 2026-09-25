//! Refinery-backed control database migrations and current-schema invariants.

use crate::control_db::ControlDb;
use crate::schema_migrations::{self, DatabaseKind};
use open_compute_core::clock::Clock;
use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::{OptionalExtension as _, Transaction};

include!(concat!(env!("OUT_DIR"), "/migration_hashes.rs"));

#[cfg(any(test, feature = "test-support"))]
const LEGACY_MIGRATIONS: &[(&str, &[u8; 32])] = &[
    ("001_init", &MIGRATION_001_SHA256),
    ("002_workers_runtime", &MIGRATION_002_SHA256),
    ("003_resource_bindings", &MIGRATION_003_SHA256),
    ("004_kv", &MIGRATION_004_SHA256),
    ("005_r2", &MIGRATION_005_SHA256),
    ("006_d1", &MIGRATION_006_SHA256),
    ("007_durable_objects", &MIGRATION_007_SHA256),
    ("008_queues", &MIGRATION_008_SHA256),
    ("009_queue_consumers", &MIGRATION_009_SHA256),
    ("010_cron_triggers", &MIGRATION_010_SHA256),
    ("011_workflows", &MIGRATION_011_SHA256),
    ("012_static_assets", &MIGRATION_012_SHA256),
    ("013_service_bindings", &MIGRATION_013_SHA256),
    ("014_cache_images", &MIGRATION_014_SHA256),
    ("015_vectorize", &MIGRATION_015_SHA256),
    ("016_ai_search", &MIGRATION_016_SHA256),
    ("017_system_owned_workers", &MIGRATION_017_SHA256),
    ("018_cloudflare_artifacts", &MIGRATION_018_SHA256),
    ("019_ai_search_r2_sources", &MIGRATION_019_SHA256),
];

/// Test-only deterministic fault injection points.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFault {
    /// Fail before Refinery begins the first transaction.
    BeforeExecution,
    /// Fail immediately after all migrations commit.
    AfterCommit,
}

/// Apply pending control migrations and verify the resulting Day 1 schema.
pub fn apply(db: &ControlDb, _clock: &dyn Clock) -> Result<(), PlatformError> {
    apply_inner(db, None)
}

/// Apply control migrations with test-only fault injection.
#[cfg(any(test, feature = "test-support"))]
pub fn apply_with_fault(
    db: &ControlDb,
    _clock: &dyn Clock,
    fault: Option<MigrationFault>,
) -> Result<(), PlatformError> {
    apply_inner(db, fault)
}

fn apply_inner(
    db: &ControlDb,
    #[cfg(any(test, feature = "test-support"))] fault: Option<MigrationFault>,
    #[cfg(not(any(test, feature = "test-support")))] _fault: Option<()>,
) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if fault == Some(MigrationFault::BeforeExecution) {
        return Err(migration_failed());
    }
    db.with_connection_mut(|connection| {
        schema_migrations::migrate(connection, DatabaseKind::Control)
    })?;
    db.with_exclusive(run_invariants)?;
    db.quick_check()?;
    #[cfg(any(test, feature = "test-support"))]
    if fault == Some(MigrationFault::AfterCommit) {
        return Err(migration_failed());
    }
    Ok(())
}

fn run_invariants(tx: &Transaction<'_>) -> Result<(), PlatformError> {
    for table in [
        "refinery_schema_history",
        "platform_meta",
        "instance_identity",
        "workers",
        "worker_versions",
        "version_vars",
        "version_secrets",
        "hostname_claims",
        "worker_host_routes",
        "public_gateway_domains",
        "public_gateway_namespaces",
        "control_idempotency",
        "version_referrers",
        "control_audit_events",
        "resources",
        "version_bindings",
        "resource_referrers",
        "kv_namespaces",
        "kv_backups",
        "r2_buckets",
        "r2_multipart_uploads",
        "r2_multipart_parts",
        "d1_databases",
        "d1_backups",
        "d1_snapshots",
        "d1_transfer_sessions",
        "d1_restore_intents",
        "do_namespaces",
        "do_objects",
        "queues",
        "queue_producer_bindings",
        "queue_referrers",
        "version_queue_consumers",
        "queue_consumers",
        "version_cron_configs",
        "version_cron_declarations",
        "cron_activations",
        "workflow_definitions",
        "workflow_versions",
        "workflow_bindings",
        "workflow_referrers",
        "workflow_instance_referrers",
        "workflow_instance_operations",
        "version_assets",
        "version_object_refs",
        "version_uploads",
        "version_upload_objects",
        "version_services",
        "version_cache_policies",
        "version_builtin_bindings",
        "system_owned_versions",
        "ai_search_r2_sources",
    ] {
        let sql: Option<String> = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| migration_failed())?;
        let Some(sql) = sql else {
            return Err(migration_failed());
        };
        if table != "refinery_schema_history" && !sql.to_ascii_uppercase().contains("STRICT") {
            return Err(migration_failed());
        }
    }
    for (index, fragment) in [
        ("instance_identity_singleton", "UNIQUE"),
        ("workers_live_name", "UNIQUE"),
        ("active_hostname_claims", "UNIQUE"),
        ("hostname_claim_authority_instance", "UNIQUE"),
        ("active_worker_origin", "UNIQUE"),
        ("resources_live_name", "tombstoned"),
        ("queues_live_name", "tombstoned"),
    ] {
        let sql: String = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
                [index],
                |row| row.get(0),
            )
            .map_err(|_| migration_failed())?;
        if !sql.contains(fragment) {
            return Err(migration_failed());
        }
    }
    let invalid_origins: bool = tx
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM workers w
                WHERE w.ownership = 'tenant' AND w.deleted_at_ms IS NULL
                  AND (SELECT COUNT(*) FROM worker_host_routes r
                       JOIN hostname_claims c ON c.id = r.claim_id
                       WHERE r.worker_id = w.id
                         AND r.exposure = 'local' AND r.state = 'active'
                         AND c.state = 'active') != 1
                UNION ALL
                SELECT 1 FROM worker_host_routes r
                JOIN hostname_claims c ON c.id = r.claim_id
                JOIN workers w ON w.id = r.worker_id
                WHERE r.state != c.state OR r.exposure != c.exposure
                   OR r.namespace != c.namespace
                   OR (r.state = 'active' AND w.deleted_at_ms IS NOT NULL)
                UNION ALL
                SELECT 1 FROM hostname_claims c
                LEFT JOIN public_gateway_domains d ON d.id = 1
                LEFT JOIN public_gateway_namespaces n
                  ON n.domain_id = d.id AND n.name = 'worker'
                WHERE c.exposure = 'public' AND c.state = 'active'
                  AND (d.id IS NULL OR n.name IS NULL
                       OR d.state IN ('disabling', 'disabled')
                       OR n.state IN ('disabling', 'disabled') OR
                       substr(c.hostname_ascii, -length(d.base_domain_ascii) - 1)
                           != '.' || d.base_domain_ascii)
                UNION ALL
                SELECT 1 FROM public_gateway_domains d
                LEFT JOIN public_gateway_namespaces n
                  ON n.domain_id = d.id AND n.name = 'worker'
                WHERE n.name IS NULL
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|_| migration_failed())?;
    if invalid_origins {
        return Err(migration_failed());
    }
    crate::workflows::integrity::verify_catalog(tx).map_err(|_| migration_failed())?;
    crate::workflows::operations::verify_operations(tx).map_err(|_| migration_failed())
}

/// Current control migration head implemented by this binary.
#[must_use]
pub fn current_schema_version() -> i64 {
    schema_migrations::current_version(DatabaseKind::Control)
}

/// Frozen identities and SHA-256 checksums of the published pre-Refinery migrations.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn legacy_migration_registry() -> Vec<(i64, &'static str, [u8; 32])> {
    LEGACY_MIGRATIONS
        .iter()
        .enumerate()
        .map(|(index, (name, checksum))| ((index + 1) as i64, *name, **checksum))
        .collect()
}

#[cfg(test)]
pub(crate) fn expected_checksum(version: i64) -> Result<&'static [u8], PlatformError> {
    usize::try_from(version)
        .ok()
        .and_then(|value| value.checked_sub(1))
        .and_then(|index| LEGACY_MIGRATIONS.get(index))
        .map(|(_, checksum)| checksum.as_slice())
        .ok_or_else(migration_failed)
}

/// Read-only control migration inspection used by doctor and snapshot validation.
pub fn inspect_schema(db: &ControlDb) -> Result<i64, PlatformError> {
    db.with_connection_mut(|connection| {
        schema_migrations::inspect(connection, DatabaseKind::Control)
    })
}

fn migration_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::MigrationFailed,
        "control database migration history or schema is invalid",
    )
}

#[cfg(test)]
#[path = "migrations_tests.rs"]
mod coverage_tests;
