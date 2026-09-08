//! Real filesystem, lock, control-database, and AEAD tests.

use crate::data_dir::{expected_directories, future_resource_paths};
use crate::fs as sfs;
use crate::master_key;
use crate::migrations::MigrationFault;
use crate::{
    CatalogDirection, CatalogSort, DataDir, IdempotencyReservation, NewQueueConsumerDeclaration,
    NewVersion, PlatformStorage, QueueConsumerConfig, QueueConsumerRepository,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
    SYSTEM_DASHBOARD_WORKER_NAME, SecretCrypto, StoredVersionSecret, SystemOwnedVersionKind,
    UpdateWorkerObservabilitySettings, VersionState, WorkerRepository, atomic_write,
    decode_catalog_cursor, inspect_durable_object_storage,
};
use open_compute_core::clock::{DeterministicClock, SystemClock};
use open_compute_core::config::DataConfig;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, HardeningConfig, ObjectStorageKind,
    PlatformReleaseIdentityV1, QueueConsumerId, ResourceId, SecretBytes, VersionId, WorkerId,
};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

mod worker_catalog_pages_cover_filters_sorts_cursors_and_deployment_state;

fn unique_root() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("data");
    (tmp, root)
}

fn restore_writable(path: &Path) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
        }
    }
}

mod clean_and_repeat_bootstrap_preserves_identity;

mod p1_control_inventory_returns_only_fixed_aggregate_counts;

mod p1_owned_schema_inspection_sees_uncheckpointed_bootstrap_wal;

mod p1_readonly_schema_fence_sees_uncheckpointed_bootstrap_wal;

mod p1_schema_inspection_checks_current_kv_and_d1_files_without_mutation;

mod p1_disk_admission_modes_and_staging_tree_validation_are_explicit;

mod durable_object_storage_marker_is_stable_and_inspectable;

mod lock_released_after_failed_bootstrap;

mod relative_and_symlink_root_rejected;

mod child_symlink_and_fifo_rejected;

mod world_writable_root_rejected;

mod filesystem_and_lock_helpers_reject_missing_special_and_escaping_paths;

mod identity_bootstrap_rejects_corrupt_existing_authority_rows;

mod exact_layout_and_no_future_files;

mod version_staging_is_private_and_crash_residue_is_cleared_under_lock;

mod atomic_replace_and_temp_cleanup;

mod pragmas_schema_strict_and_partial_index;

fn raw_user_version(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

mod migration_faults_checksum_future_and_restart;

mod p0_2_migration_ddl_fault_rolls_back_to_schema_one;

mod master_key_modes_and_failures;

mod db_fingerprint_mismatch_fails_closed;

mod product_cursor_hmacs_are_domain_separated_and_reject_tampering;

mod no_secrets_in_db_debug_json_or_errors;

mod lock_metadata_is_diagnostic_only;

mod subprocess_lock_contention;

mod readonly_root_rejects_mutation;

mod durability_does_not_claim_safety_when_unclassified;

mod partially_created_key_is_rejected;

mod lock_symlink_and_loose_mode_are_rejected_without_side_effects;

mod sqlite_and_key_symlinks_are_rejected;

mod missing_master_key_env_fails_closed;

mod key_file_rejects_trailing_bytes_and_does_not_overwrite;

mod seeded_inconsistent_migrations_fail_closed;

mod incomplete_identity_fails_without_mutation;

mod inspect_lock_holds_and_releases_flock;

mod operation_receipt_reads_are_bounded_and_never_follow_symlinks;

mod inspect_control_db_accepts_uri_special_path_chars;

mod snapshot_version_artifact_inventory_uses_the_canonical_sharded_key;

mod p0_2_repository_enforces_lifecycle_immutability_and_idempotency;

mod p0_2_delete_referrer_recovery_and_worker_identity_are_fenced;

mod p0_2_concurrent_promotions_have_one_linearization_winner;

mod filesystem_helpers_cover_secure_success_and_failure_paths;

mod control_db_read_write_helpers_and_failures_are_enforced;

mod master_key_inspection_rejects_missing_malformed_and_ambiguous_sources;

mod master_key_inspection_covers_env_file_utf8_and_mismatch_paths;

mod master_key_process_environment_modes_are_covered_in_isolated_processes;

mod inspection_layout_migration_and_repository_helpers_are_covered;

fn inspect_identity_after(sql: &str) -> ErrorCode {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "ignore_check_constraints", "ON")
        .unwrap();
    conn.execute_batch(sql).unwrap();
    drop(conn);
    let db = crate::ControlDb::open(&root.join("control.sqlite"), 100).unwrap();
    crate::identity::inspect_stored(&db).unwrap_err().code()
}

mod inspect_stored_identity_rejects_every_malformed_authority_field;

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "the test callback signature matches the exercised API"
)]
fn insert_ready(
    repo: &WorkerRepository<'_>,
    account: AccountId,
    worker: WorkerId,
    digest: [u8; 32],
    request: open_compute_core::RequestId,
    now: i64,
) -> VersionId {
    let id = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id,
            account_id: account,
            worker_id: worker,
            content_kind: crate::VersionContentKind::Worker,
            artifact_sha256: Some(digest),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: digest,
            compatibility_date: "2026-08-30".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: now,
        },
        &crate::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    repo.begin_validation(id).unwrap();
    repo.mark_ready(id, now + 1).unwrap();
    id
}

mod service_declarations_follow_active_targets_and_protect_worker_identity;

mod queue_consumer_unique_index_serializes_concurrent_worker_attachments;

mod api_queue_consumer_generations_preserve_version_manifest_and_release_queue_refs;

mod worker_repository_rejects_invalid_state_and_ownership_operations;

fn inspect_schema_after_raw_sql(sql: &str) -> ErrorCode {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("control.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(sql).unwrap();
    drop(conn);
    let db = crate::ControlDb::open(&path, 100).unwrap();
    crate::migrations::inspect_schema(&db).unwrap_err().code()
}

mod schema_consistency_rejects_missing_malformed_and_duplicate_rows;

mod bootstrap_with_no_fault_matches_normal_bootstrap_and_rejects_nonregular_staging;

mod control_db_operations_fail_closed_when_foreign_keys_are_disabled;

fn p1_release_identity() -> PlatformReleaseIdentityV1 {
    PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: env!("CARGO_PKG_VERSION").to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "a".repeat(64),
        runtime_assets_sha256: "b".repeat(64),
        dashboard_assets_sha256: "c".repeat(64),
        facade_capability_version: 1,
        control_schema_version: u32::try_from(crate::migrations::current_schema_version()).unwrap(),
        scheduler_schema_version: u32::try_from(crate::current_scheduler_schema_version()).unwrap(),
        kv_schema_version: crate::KV_SCHEMA_VERSION,
        d1_schema_version: crate::D1_DATABASE_SCHEMA_VERSION,
        vectorize_schema_version: crate::vectorize::VECTORIZE_SCHEMA_VERSION,
        ai_search_schema_version: crate::ai_search::AI_SEARCH_SCHEMA_VERSION,
        snapshot_format_version: 1,
    }
}

mod p1_offline_snapshot_is_standalone_authenticated_and_rejects_do_symlinks;

mod p1_admission_lock_restore_target_and_current_schema_fail_closed;

mod p2_2_queue_catalog_projection_and_config_fences_are_exact;

mod p2_2_queue_enqueue_delay_quota_retention_and_counters_are_transactional;

mod p2_2_concurrent_queue_enqueues_never_exceed_backlog_quota;

mod p2_2_queue_catalog_idempotency_and_failure_boundaries_are_complete;

mod p1_restore_cleanup_rejects_ambiguous_bounds_links_receipts_and_lock_owners;

mod p1_concurrent_resource_creates_never_exceed_the_account_kind_limit;

mod system_dashboard_worker_is_excluded_from_tenant_catalog_and_mutations;

mod worker_observability_settings_are_day1_authority_and_invalidate_runtime_generation;
