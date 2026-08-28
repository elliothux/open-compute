//! Real two-database lifecycle operations, including recovery between committed saga phases.

use super::*;
use open_compute_core::WorkflowOperationId;
use open_compute_core::workflow::{WorkflowRetention, WorkflowStepDeclaration, WorkflowStepKind};
use open_compute_storage::scheduler::{
    WorkflowInstanceAction, WorkflowStepAttempt, WorkflowStepOutcome, WorkflowV2StepGrant,
};
use open_compute_storage::{WorkflowOperationKind, WorkflowOperationResult};

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
    let deployment = repo.version(account, current).unwrap().target.deployment_id;
    let version = repo
        .stage_version(account, definition, deployment, "Flow", 2, 5)
        .unwrap();
    repo.finish_version(account, version.target.version_id, true, 6)
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
            2,
            Some("reusable"),
            WorkflowCreateInput {
                payload_json: r#"{"value":7}"#,
                retention: Some(&WorkflowRetention {
                    success_retention_ms: 3600000,
                    error_retention_ms: 3600000,
                }),
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
        batch_first_ordinal: 0,
        batch_size: 1,
        dependencies: vec![],
    }
    .resolve()
    .unwrap();
    let WorkflowV2StepGrant::Run {
        step_token,
        attempt,
        ..
    } = store
        .claim_workflow_batch_v2(&run.fence, &[step], config.dispatch_timeout_ms, now, config)
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

#[test]
fn durable_purge_receipts_cannot_be_erased_by_corrupt_scheduler_recovery() {
    for finalized in [false, true] {
        let (_temp, storage, scheduler, definition) = durable_fixture();
        let config = WorkflowsConfig::default();
        let account = storage.identity().default_account_id;
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        let identity = create(&controller, account, definition, 10);
        controller
            .modify(
                account,
                definition,
                identity.instance_id,
                WorkflowInstanceAction::Terminate,
                11,
            )
            .unwrap();
        let repo = WorkflowRepository::new(storage.db());
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                &config,
                3600011,
            )
            .unwrap();
        let WorkflowOperationResult::Applied(proof) = scheduler
            .apply_workflow_operation(&operation, 3600011, &config)
            .unwrap()
        else {
            panic!("purge");
        };
        if finalized {
            repo.complete_instance_operation(&proof, 3600012).unwrap();
            assert!(repo.reservation(identity.instance_id).unwrap().is_none());
            assert!(repo.instance_operations(None, 10).unwrap().is_empty());
            assert!(!repo.instance_referrers_intact(&identity).unwrap());
        }
        assert_eq!(scheduler.workflow_gc_receipts(None, 10).unwrap().len(), 1);
        drop(scheduler);
        let path = storage.data_dir().scheduler_db_path();
        std::fs::write(&path, b"corrupt durable workflow receipt").unwrap();
        assert_eq!(
            storage
                .data_dir()
                .recover_corrupt_scheduler_db("scheduler-corrupt-durable", 5000, 3600013)
                .unwrap_err()
                .code(),
            ErrorCode::SchedulerUnavailable
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"corrupt durable workflow receipt"
        );
        assert!(
            !storage
                .data_dir()
                .root()
                .join("diagnostics/scheduler-recovery/scheduler-corrupt-durable")
                .exists()
        );
    }
}

#[test]
fn operator_retention_defaults_affect_only_new_instances_even_after_restart() {
    let (_temp, storage, scheduler, definition) = durable_fixture();
    let account = storage.identity().default_account_id;
    let mut limits = WorkflowsConfig {
        default_retention: WorkflowRetention {
            success_retention_ms: 3600000,
            error_retention_ms: 7200000,
        },
        ..Default::default()
    };
    let old = WorkflowController::new(&storage, &scheduler, &limits)
        .create(
            account,
            definition,
            2,
            None,
            WorkflowCreateInput {
                payload_json: "null",
                retention: None,
            },
            10,
        )
        .unwrap();
    let frozen = limits.default_retention.clone();
    limits.default_retention = WorkflowRetention {
        success_retention_ms: 10800000,
        error_retention_ms: 14400000,
    };
    let path = storage.data_dir().scheduler_db_path();
    drop(scheduler);
    let scheduler = SchedulerStore::open(&path, 5000, 11).unwrap();
    let controller = WorkflowController::new(&storage, &scheduler, &limits);
    let new = controller
        .create(
            account,
            definition,
            2,
            None,
            WorkflowCreateInput {
                payload_json: "null",
                retention: None,
            },
            11,
        )
        .unwrap();
    assert_eq!(
        scheduler
            .workflow_instance(new.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .unwrap()
            .retention,
        limits.default_retention
    );
    controller
        .modify(
            account,
            definition,
            old.instance_id,
            WorkflowInstanceAction::Terminate,
            12,
        )
        .unwrap();
    let retained = scheduler
        .workflow_instance(old.instance_id)
        .unwrap()
        .unwrap()
        .durable
        .unwrap();
    assert_eq!(retained.retention, frozen);
    assert_eq!(retained.expires_at_ms, Some(7200012));
    controller
        .restart(
            account,
            definition,
            old.instance_id,
            WorkflowOperationId::generate(),
            13,
        )
        .unwrap();
    assert_eq!(
        scheduler
            .workflow_instance(old.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .unwrap()
            .retention,
        frozen
    );
}

#[test]
fn restart_saga_replays_each_committed_phase_and_preserves_frozen_version() {
    for phase in 0..=2 {
        let (_temp, storage, mut scheduler, definition) = durable_fixture();
        let account = storage.identity().default_account_id;
        let config = WorkflowsConfig::default();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        let identity = create(&controller, account, definition, 10);
        let old = controller
            .claim(11, &mut Default::default())
            .unwrap()
            .unwrap();
        let old_grant = grant(&scheduler, &old, 11, &config);
        controller
            .send_event(
                account,
                definition,
                identity.instance_id,
                "approval",
                "true",
                12,
            )
            .unwrap();
        let repo = WorkflowRepository::new(storage.db());
        let newer = repo
            .stage_version(
                account,
                definition,
                identity.target.deployment_id,
                "Flow",
                2,
                13,
            )
            .unwrap();
        repo.finish_version(account, newer.target.version_id, true, 13)
            .unwrap();
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Restart,
                &config,
                14,
            )
            .unwrap();
        assert_eq!(
            repo.prepare_instance_operation(
                &identity,
                operation.id(),
                WorkflowOperationKind::Restart,
                &config,
                14
            )
            .unwrap(),
            operation
        );
        assert_eq!(
            repo.prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Restart,
                &config,
                14
            )
            .unwrap_err()
            .code(),
            ErrorCode::WorkflowInstanceBusy
        );
        assert_eq!(
            controller
                .status(account, definition, identity.instance_id, 2, 14)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceBusy
        );
        assert_eq!(
            controller
                .send_event(
                    account,
                    definition,
                    identity.instance_id,
                    "approval",
                    "true",
                    14
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceBusy
        );
        if phase >= 1 {
            let WorkflowOperationResult::Applied(proof) = scheduler
                .apply_workflow_operation(&operation, 15, &config)
                .unwrap()
            else {
                panic!("restart")
            };
            assert!(
                controller
                    .claim(15, &mut Default::default())
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                scheduler
                    .settle_workflow_step_v2(
                        &old.fence,
                        &old_grant,
                        WorkflowStepOutcome::Success("true"),
                        15,
                        &config
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowRunStale
            );
            if phase == 2 {
                repo.complete_instance_operation(&proof, 16).unwrap();
            }
        }
        assert_eq!(inspect(&storage).pending_restarts, u64::from(phase < 2));
        let path = storage.data_dir().ensure_scheduler_db().unwrap();
        drop(scheduler);
        scheduler = SchedulerStore::open(&path, 5000, 17).unwrap();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, 17)
            .unwrap();
        let next = scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(next.identity.instance_generation, 2);
        assert_eq!(next.identity.target, identity.target);
        assert_ne!(next.identity.target.version_id, newer.target.version_id);
        assert_eq!(next.identity.created_at_ms, 10);
        assert_eq!(next.input_json, r#"{"value":7}"#);
        assert_eq!(next.durable.as_ref().unwrap().registered_step_count, 0);
        assert_eq!(next.durable.as_ref().unwrap().event_count, 0);
        assert_eq!(next.durable.as_ref().unwrap().next_event_seq, 1);
        assert_eq!(
            repo.reservation(identity.instance_id)
                .unwrap()
                .unwrap()
                .identity,
            next.identity
        );
        assert!(
            repo.instance_operation(identity.instance_id)
                .unwrap()
                .is_none()
        );
        let run = controller
            .claim(18, &mut Default::default())
            .unwrap()
            .unwrap();
        let step = grant(&scheduler, &run, 18, &config);
        scheduler
            .settle_workflow_step_v2(
                &run.fence,
                &step,
                WorkflowStepOutcome::Success("8"),
                19,
                &config,
            )
            .unwrap();
        assert!(matches!(
            scheduler
                .apply_workflow_operation(&operation, 20, &config)
                .unwrap(),
            WorkflowOperationResult::Applied(_)
        ));
        assert_eq!(
            scheduler
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap()
                .completed_step_count,
            1
        );
        controller
            .restart(
                account,
                definition,
                identity.instance_id,
                WorkflowOperationId::generate(),
                21,
            )
            .unwrap();
        assert_eq!(
            scheduler
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap()
                .identity
                .instance_generation,
            3
        );
        assert_eq!(
            scheduler
                .apply_workflow_operation(&operation, 22, &config)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowRunStale
        );
        scheduler
            .verify_workflow_history(identity.instance_id)
            .unwrap();
        repo.verify_catalog().unwrap();
    }
}

#[test]
fn rejected_restart_is_durable_and_cannot_revive_after_clock_regression() {
    let (_temp, storage, scheduler, definition) = durable_fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    let identity = create(&controller, account, definition, 10);
    controller
        .modify(
            account,
            definition,
            identity.instance_id,
            WorkflowInstanceAction::Terminate,
            20,
        )
        .unwrap();
    let repo = WorkflowRepository::new(storage.db());
    let expiry = 3600020;
    let operation = repo
        .prepare_instance_operation(
            &identity,
            WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &config,
            expiry - 1,
        )
        .unwrap();
    let WorkflowOperationResult::Rejected(proof) = scheduler
        .apply_workflow_operation(&operation, expiry, &config)
        .unwrap()
    else {
        panic!("expiry rejection")
    };
    assert_eq!(proof.code(), ErrorCode::WorkflowInstanceNotFound);
    assert_eq!(
        repo.reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Restarting
    );
    let path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(scheduler);
    let scheduler = SchedulerStore::open(&path, 5000, expiry).unwrap();
    let WorkflowOperationResult::Rejected(proof) = scheduler
        .apply_workflow_operation(&operation, expiry - 100, &config)
        .unwrap()
    else {
        panic!("rejection must survive clock regression")
    };
    repo.cancel_instance_operation(&proof, expiry).unwrap();
    repo.cancel_instance_operation(&proof, expiry).unwrap();
    assert_eq!(
        repo.reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Retained
    );
    let next = repo
        .prepare_instance_operation(
            &identity,
            WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &config,
            expiry - 90,
        )
        .unwrap();
    assert_eq!(next.sequence(), operation.sequence() + 1);
    let WorkflowOperationResult::Applied(proof) = scheduler
        .apply_workflow_operation(&next, expiry - 90, &config)
        .unwrap()
    else {
        panic!("new request")
    };
    repo.complete_instance_operation(&proof, expiry - 90)
        .unwrap();
    assert_eq!(
        scheduler
            .apply_workflow_operation(&operation, expiry - 200, &config)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    repo.verify_catalog().unwrap();
    scheduler
        .verify_workflow_history(identity.instance_id)
        .unwrap();
}

#[test]
fn purge_saga_keeps_references_until_proof_and_only_then_reuses_the_public_id() {
    for phase in 0..=3 {
        let (_temp, storage, scheduler, definition) = durable_fixture();
        let account = storage.identity().default_account_id;
        let config = WorkflowsConfig::default();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        let identity = create(&controller, account, definition, 10);
        controller
            .modify(
                account,
                definition,
                identity.instance_id,
                WorkflowInstanceAction::Terminate,
                20,
            )
            .unwrap();
        let expiry = 3600020;
        assert!(matches!(
            controller
                .status(account, definition, identity.instance_id, 2, expiry - 1)
                .unwrap(),
            WorkflowStatus::Terminated
        ));
        assert_eq!(
            controller
                .status(account, definition, identity.instance_id, 2, expiry)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceNotFound
        );
        assert_eq!(
            controller
                .create(
                    account,
                    definition,
                    2,
                    Some("reusable"),
                    WorkflowCreateInput {
                        payload_json: "{}",
                        retention: None
                    },
                    expiry
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceCleanupPending
        );
        let repo = WorkflowRepository::new(storage.db());
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                &config,
                expiry,
            )
            .unwrap();
        if phase >= 1 {
            let WorkflowOperationResult::Applied(proof) = scheduler
                .apply_workflow_operation(&operation, expiry, &config)
                .unwrap()
            else {
                panic!("purge")
            };
            assert!(
                scheduler
                    .workflow_instance(identity.instance_id)
                    .unwrap()
                    .is_none()
            );
            assert!(repo.instance_referrers_intact(&identity).unwrap());
            let receipt = scheduler.workflow_gc_receipts(None, 10).unwrap().remove(0);
            assert_eq!(
                repo.acknowledge_workflow_gc(&receipt).unwrap_err().code(),
                ErrorCode::WorkflowInstanceBusy
            );
            if phase >= 2 {
                repo.complete_instance_operation(&proof, expiry).unwrap();
                if phase == 3 {
                    scheduler
                        .sweep_workflow_gc(&repo.acknowledge_workflow_gc(&receipt).unwrap())
                        .unwrap();
                }
            }
        }
        let diagnostics = inspect(&storage);
        assert_eq!(diagnostics.pending_purges, u64::from(phase < 2));
        assert_eq!(diagnostics.pending_receipt_sweeps, u64::from(phase == 2));
        let path = storage.data_dir().ensure_scheduler_db().unwrap();
        drop(scheduler);
        let scheduler = SchedulerStore::open(&path, 5000, expiry + 1).unwrap();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, expiry + 1)
            .unwrap();
        assert!(repo.reservation(identity.instance_id).unwrap().is_none());
        assert!(!repo.instance_referrers_intact(&identity).unwrap());
        assert!(scheduler.workflow_gc_receipts(None, 10).unwrap().is_empty());
        let next = create(&controller, account, definition, expiry + 2);
        assert_ne!(next.instance_id, identity.instance_id);
        assert_ne!(next.creation_nonce, identity.creation_nonce);
        assert_eq!(
            controller
                .send_event(
                    account,
                    definition,
                    identity.instance_id,
                    "approval",
                    "true",
                    expiry + 3
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceNotFound
        );
        controller
            .send_event(
                account,
                definition,
                next.instance_id,
                "approval",
                "true",
                expiry + 3,
            )
            .unwrap();
        repo.verify_catalog().unwrap();
        scheduler.verify_workflow_history(next.instance_id).unwrap();
    }
}
