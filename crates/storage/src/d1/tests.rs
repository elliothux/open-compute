use super::engine::{map_internal_error, map_open_error};
use super::*;
use crate::crypto::SecretCrypto;
use crate::master_key;
use open_compute_core::{AccountId, D1Config, ErrorCode, ResourceId, SecretBytes};
use sha2::{Digest, Sha256};

struct Fixture {
    _temp: tempfile::TempDir,
    engine: D1Engine,
    account: AccountId,
    resource: ResourceId,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    let engine = D1Engine::create(
        &temp.path().join("data.sqlite"),
        account,
        resource,
        1_700_000_000_000,
        64 * 1024 * 1024,
    )
    .unwrap();
    Fixture {
        _temp: temp,
        engine,
        account,
        resource,
    }
}

fn limits() -> D1QueryLimits {
    D1QueryLimits::query(&D1Config::default()).unwrap()
}

fn statement(sql: &str, params: Vec<D1Value>) -> D1Statement {
    D1Statement {
        sql: sql.to_owned(),
        params,
    }
}

mod create_crud_types_and_raw_column_order;

mod parameter_and_statement_limits_fail_closed;

mod sqlite_and_materialization_limits_enforce_exact_boundaries;

mod progress_handler_distinguishes_vm_budget_and_wall_timeout;

mod frozen_database_quota_returns_database_full;

mod session_version_is_monotonic_across_writes_reads_and_restore;

mod batch_is_atomic_and_ordered;

mod exec_uses_sqlite_tail_parser_and_versions_a_committed_prefix;

mod exec_persists_no_write_when_the_history_bump_fails;

mod exec_commit_failure_keeps_data_rolled_back_and_head_reconcilable;

mod authorizer_blocks_cross_database_internal_and_connection_state_sql;

mod migration_ledger_is_atomic_idempotent_and_detects_drift;

mod migration_gap_failure_and_authorizer_rollback_without_ledger_rows;

mod online_backup_and_restore_rewrite_identity_but_keep_tenant_data;

mod separate_database_files_are_isolated;

mod wal_recovery_keeps_committed_and_discards_uncommitted_transaction;

mod corrupt_database_is_local_and_does_not_block_another_file;

mod wal_observation_accepts_owned_sidecar_and_rejects_symlink;

mod engine_rejects_invalid_creation_and_maps_sqlite_failures_stably;
