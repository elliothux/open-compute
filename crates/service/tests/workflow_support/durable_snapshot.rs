//! Snapshot coverage for durable waits and every cross-database operation handoff.

use super::*;
use open_compute_core::workflow::{WorkflowRetention, WorkflowStepDeclaration, WorkflowStepKind};
use open_compute_core::{DeploymentId, WorkflowOperationId};
use open_compute_storage::scheduler::{WorkflowInstanceAction, WorkflowState};
use open_compute_storage::{
    WorkflowInstanceIdentity, WorkflowOperationKind, WorkflowOperationResult,
};

pub(super) struct Case {
    identity: WorkflowInstanceIdentity,
    mode: &'static str,
    deadline: Option<i64>,
}

pub(super) fn prepare(
    storage: &PlatformStorage,
    scheduler: &SchedulerStore,
    config: &WorkflowsConfig,
    deployment: DeploymentId,
) -> Vec<Case> {
    let base = now() - 7_200_000;
    let account = storage.identity().default_account_id;
    let repo = WorkflowRepository::new(storage.db());
    let definition = repo
        .create_definition(account, "durable-snapshot", base)
        .unwrap();
    // The snapshot fixture exercises real storage APIs; class execution is covered by the
    // stock-workerd driver Gate, not by manufacturing callback results in this fixture.
    let version = repo
        .stage_version(account, definition.id, deployment, "Flow", base)
        .unwrap();
    repo.finish_version(account, version.target.version_id, true, base)
        .unwrap();
    let controller = WorkflowController::new(storage, scheduler, config);
    let mut cases = Vec::new();
    for mode in [
        "sleep",
        "paused",
        "inbox",
        "restart-prepared",
        "restart-applied",
        "purge-prepared",
        "purge-deleted",
        "purge-released",
    ] {
        let identity = controller
            .create(
                account,
                definition.id,
                WorkflowOperationId::generate(),
                Some(mode),
                open_compute_workers::WorkflowCreateInput {
                    payload_base64: &encode_workflow_json(&serde_json::json!({"snapshot":true})),
                    retention: Some(&WorkflowRetention {
                        success_retention_ms: 3600000,
                        error_retention_ms: 3600000,
                    }),
                    schedule: None,
                },
                base + 1,
            )
            .unwrap();
        let mut deadline = None;
        if matches!(mode, "sleep" | "paused" | "inbox") {
            let run = scheduler
                .claim_workflow(&identity, base + 2, config)
                .unwrap()
                .unwrap();
            if mode == "inbox" {
                controller
                    .send_event(
                        account,
                        definition.id,
                        identity.instance_id,
                        open_compute_workers::WorkflowEventInput {
                            operation_id: WorkflowOperationId::generate(),
                            event_type: "unmatched",
                            payload_base64: &encode_workflow_json(
                                &serde_json::json!({"kept":true}),
                            ),
                        },
                        base + 3,
                    )
                    .unwrap();
            }
            let step = WorkflowStepDeclaration {
                ordinal: 0,
                name: "wait".into(),
                name_count: 1,
                kind: if mode == "inbox" {
                    WorkflowStepKind::WaitEvent
                } else {
                    WorkflowStepKind::Sleep
                },
                config: if mode == "inbox" {
                    serde_json::json!({"type":"approval","timeout":86400000})
                } else {
                    serde_json::json!({"duration":86400000})
                },
                rollback_config: None,
                rollback_step: false,
                dependencies: vec![],
                batch_first_ordinal: 0,
                batch_size: 1,
            }
            .resolve()
            .unwrap();
            scheduler
                .claim_workflow_batch(
                    &run.fence,
                    std::slice::from_ref(&step),
                    config.dispatch_timeout_ms,
                    base + 4,
                    config,
                )
                .unwrap();
            scheduler.yield_workflow(&run.fence, base + 5).unwrap();
            if mode == "paused" {
                controller
                    .modify(
                        account,
                        definition.id,
                        identity.instance_id,
                        WorkflowInstanceAction::Pause,
                        base + 6,
                    )
                    .unwrap();
            }
            deadline = scheduler
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap()
                .durable
                .next_wake_at_ms;
        } else {
            let purge = mode.starts_with("purge");
            if purge {
                controller
                    .modify(
                        account,
                        definition.id,
                        identity.instance_id,
                        WorkflowInstanceAction::Terminate,
                        base + 2,
                    )
                    .unwrap();
            }
            let operation = repo
                .prepare_instance_operation(
                    &identity,
                    WorkflowOperationId::generate(),
                    if purge {
                        WorkflowOperationKind::Purge
                    } else {
                        WorkflowOperationKind::Restart
                    },
                    config,
                    base + 3600003,
                )
                .unwrap();
            if !mode.ends_with("prepared") {
                let WorkflowOperationResult::Applied(proof) = scheduler
                    .apply_workflow_operation(&operation, base + 3600003, config)
                    .unwrap()
                else {
                    panic!("operation");
                };
                if mode == "purge-released" {
                    repo.complete_instance_operation(&proof, base + 3600004)
                        .unwrap();
                }
            }
        }
        cases.push(Case {
            identity,
            mode,
            deadline,
        });
    }
    assert_eq!(scheduler.workflow_gc_receipts(None, 32).unwrap().len(), 2);
    assert_eq!(repo.inspect_operations().unwrap().pending_purges, 2);
    cases
}

pub(super) fn verify(
    storage: &PlatformStorage,
    scheduler: &SchedulerStore,
    config: &WorkflowsConfig,
    cases: &[Case],
    now_ms: i64,
) {
    let repo = WorkflowRepository::new(storage.db());
    let controller = WorkflowController::new(storage, scheduler, config);
    // Both proofs and the receipt-only P3 state must survive the copy before reconciliation.
    assert_eq!(repo.inspect_operations().unwrap().pending_restarts, 2);
    assert_eq!(scheduler.workflow_gc_receipts(None, 32).unwrap().len(), 2);
    let diagnostic = open_compute_storage::scheduler::inspect_workflow_databases(
        &storage.data_dir().control_db_path(),
        &storage.data_dir().scheduler_db_path(),
        5000,
        100,
    )
    .unwrap();
    assert!(diagnostic.is_valid(), "{diagnostic:?}");
    controller
        .reconcile(&mut Default::default(), 100, now_ms)
        .unwrap();
    for case in cases {
        let identity = &case.identity;
        if case.mode.starts_with("purge") {
            assert!(
                scheduler
                    .workflow_instance(identity.instance_id)
                    .unwrap()
                    .is_none()
            );
            assert!(repo.reservation(identity.instance_id).unwrap().is_none());
            assert!(!repo.instance_referrers_intact(identity).unwrap());
            continue;
        }
        let record = scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.identity.target, identity.target);
        assert!(record.run_token.is_none());
        if case.mode.starts_with("restart") {
            assert_eq!(record.identity.instance_generation, 2);
            assert_eq!(record.state, WorkflowState::Queued);
            controller
                .modify(
                    identity.target.account_id,
                    identity.target.definition_id,
                    identity.instance_id,
                    WorkflowInstanceAction::Pause,
                    now_ms,
                )
                .unwrap();
        } else {
            assert_eq!(
                record.state,
                if case.mode == "paused" {
                    WorkflowState::Paused
                } else {
                    WorkflowState::Waiting
                }
            );
            assert_eq!(record.durable.next_wake_at_ms, case.deadline);
            if case.mode == "inbox" {
                assert_eq!(record.durable.event_count, 1);
                controller
                    .send_event(
                        identity.target.account_id,
                        identity.target.definition_id,
                        identity.instance_id,
                        open_compute_workers::WorkflowEventInput {
                            operation_id: WorkflowOperationId::generate(),
                            event_type: "approval",
                            payload_base64: "T0NEVgECAw==",
                        },
                        now_ms,
                    )
                    .unwrap();
                let woken = scheduler
                    .workflow_instance(identity.instance_id)
                    .unwrap()
                    .unwrap();
                assert_eq!(woken.state, WorkflowState::Queued);
                assert_eq!(woken.durable.event_count, 1);
                controller
                    .modify(
                        identity.target.account_id,
                        identity.target.definition_id,
                        identity.instance_id,
                        WorkflowInstanceAction::Pause,
                        now_ms,
                    )
                    .unwrap();
            }
        }
        scheduler
            .verify_workflow_history(identity.instance_id)
            .unwrap();
    }
    assert!(repo.instance_operations(None, 100).unwrap().is_empty());
    assert!(
        scheduler
            .workflow_gc_receipts(None, 100)
            .unwrap()
            .is_empty()
    );
}
