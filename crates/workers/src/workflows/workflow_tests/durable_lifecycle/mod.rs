//! Real two-database lifecycle operations, including recovery between committed saga phases.

use super::*;
use open_compute_core::WorkflowOperationId;
use open_compute_core::workflow::{
    WorkflowRestartSelector, WorkflowRestartStepType, WorkflowRetention, WorkflowStepDeclaration,
    WorkflowStepKind,
};
use open_compute_storage::scheduler::{
    WorkflowInstanceAction, WorkflowStepAttempt, WorkflowStepGrant, WorkflowStepOutcome,
    WorkflowStepResult,
};
use open_compute_storage::{WorkflowOperationKind, WorkflowOperationResult};

const NULL_VALUE: &str = "T0NEVgECAA==";
const TRUE_VALUE: &str = "T0NEVgECAw==";
const EIGHT_VALUE: &str = "T0NEVgECBEAgAAAAAAAA";
const EMPTY_OBJECT_VALUE: &str = "T0NEVgECEQAAAAA=";
pub(super) const OBJECT_VALUE: &str = "T0NEVgECEQAAAAEAAAAFdmFsdWUEQBwAAAAAAAA=";

fn inspect(
    storage: &PlatformStorage,
) -> open_compute_storage::scheduler::WorkflowDatabaseInspection {
    let value = open_compute_storage::scheduler::inspect_workflow_databases(
        &storage.data_dir().root().join("control.sqlite"),
        &storage.data_dir().scheduler_db_path(),
        5000,
        32,
    )
    .unwrap();
    assert!(value.is_valid(), "{value:?}");
    value
}

pub(super) fn durable_fixture() -> (
    tempfile::TempDir,
    PlatformStorage,
    SchedulerStore,
    WorkflowId,
) {
    let (temp, storage, scheduler, definition) = fixture();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let current = repo
        .definition(account, definition)
        .unwrap()
        .current_version_id
        .unwrap();
    let version = repo
        .version(account, current)
        .unwrap()
        .target
        .worker_version_id;
    let version = repo
        .stage_version(account, definition, version, "Flow", 5)
        .unwrap();
    repo.finish_version(account, version.target.workflow_version_id, true, 6)
        .unwrap();
    (temp, storage, scheduler, definition)
}

pub(super) fn create(
    controller: &WorkflowController<'_>,
    account: AccountId,
    definition: WorkflowId,
    now: i64,
) -> WorkflowInstanceIdentity {
    controller
        .create(
            account,
            definition,
            WorkflowOperationId::generate(),
            Some("reusable"),
            WorkflowCreateInput {
                payload_base64: OBJECT_VALUE,
                retention: Some(&WorkflowRetention {
                    success_retention_ms: 3600000,
                    error_retention_ms: 3600000,
                }),
                schedule: None,
            },
            now,
        )
        .unwrap()
}

pub(super) fn grant(
    store: &SchedulerStore,
    run: &ClaimedWorkflowRun,
    now: i64,
    config: &WorkflowsConfig,
) -> WorkflowStepAttempt {
    let step = WorkflowStepDeclaration {
        ordinal: 0,
        name: "effect".into(),
        name_count: 1,
        kind: WorkflowStepKind::Do,
        config: serde_json::json!({"timeout":1000,"retries":{"limit":0,"delay":0}}),
        rollback_config: None,
        rollback_step: false,
        batch_first_ordinal: 0,
        batch_size: 1,
        dependencies: vec![],
    }
    .resolve()
    .unwrap();
    let WorkflowStepGrant::Run {
        step_token,
        attempt,
        ..
    } = store
        .claim_workflow_batch(&run.fence, &[step], config.dispatch_timeout_ms, now, config)
        .unwrap()
        .remove(0)
    else {
        panic!("grant")
    };
    WorkflowStepAttempt {
        ordinal: 0,
        attempt,
        step_token,
    }
}

fn attempt(ordinal: u32, grant: WorkflowStepGrant) -> WorkflowStepAttempt {
    let WorkflowStepGrant::Run {
        step_token,
        attempt,
        ..
    } = grant
    else {
        panic!("expected callback grant")
    };
    WorkflowStepAttempt {
        ordinal,
        attempt,
        step_token,
    }
}

mod durable_purge_receipts_cannot_be_erased_by_corrupt_scheduler_recovery;

mod operator_retention_defaults_affect_only_new_instances_even_after_restart;

mod restart_saga_replays_each_committed_phase_and_preserves_frozen_version;

mod restart_from_reconciles_response_loss_and_replays_only_the_exact_completed_prefix;

mod rejected_restart_is_durable_and_cannot_revive_after_clock_regression;

mod purge_saga_keeps_references_until_proof_and_only_then_reuses_the_public_id;
