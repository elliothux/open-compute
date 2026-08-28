//! SIGKILL covers V2 durable decisions and both-database lifecycle ownership.

use super::super::durable_lifecycle::{create, durable_fixture};
use super::*;
use open_compute_core::{
    WorkflowOperationId,
    workflow::{WorkflowStepDeclaration, WorkflowStepDescriptor, WorkflowStepKind},
};
use open_compute_storage::scheduler::{
    WorkflowInstanceAction, WorkflowStepAttempt, WorkflowStepOutcome, WorkflowV2StepGrant,
};
use open_compute_storage::{WorkflowOperationKind, WorkflowOperationResult};

const DURABLE_CHILD: &str = "workflows::tests::crash_matrix::durable::workflow_durable_crash_child";

fn descriptor() -> WorkflowStepDescriptor {
    WorkflowStepDeclaration {
        ordinal: 0,
        name: "effect".into(),
        name_count: 1,
        kind: WorkflowStepKind::Do,
        config: serde_json::json!({"timeout":1000,"retries":{"limit":1,"delay":100}}),
        batch_first_ordinal: 0,
        batch_size: 1,
        dependencies: vec![],
    }
    .resolve()
    .unwrap()
}

#[test]
fn workflow_durable_crash_child() {
    let Ok(root) = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_DATA") else {
        return;
    };
    let cut = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_POINT").unwrap();
    let definition = std::env::var("OPEN_COMPUTE_WORKFLOW_CRASH_DEFINITION")
        .unwrap()
        .parse()
        .unwrap();
    let storage =
        PlatformStorage::bootstrap(&storage_config(Path::new(&root)), &SystemClock).unwrap();
    let scheduler =
        SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5000, 10).unwrap();
    let account = storage.identity().default_account_id;
    let limits = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &limits);
    let identity = create(&controller, account, definition, 10);
    let repo = WorkflowRepository::new(storage.db());
    let run = controller
        .claim(11, &mut Default::default())
        .unwrap()
        .unwrap();
    let WorkflowV2StepGrant::Run {
        step_token,
        attempt,
        ..
    } = scheduler
        .claim_workflow_batch_v2(
            &run.fence,
            &[descriptor()],
            limits.dispatch_timeout_ms,
            12,
            &limits,
        )
        .unwrap()
        .remove(0)
    else {
        panic!("grant")
    };
    let attempt = WorkflowStepAttempt {
        ordinal: 0,
        attempt,
        step_token,
    };
    checkpoint(&cut, "attempt-granted");
    if cut == "retry-committed" {
        scheduler
            .settle_workflow_step_v2(
                &run.fence,
                &attempt,
                WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),
                13,
                &limits,
            )
            .unwrap();
        checkpoint(&cut, "retry-committed");
    }
    if cut == "pause-requested" {
        controller
            .modify(
                account,
                definition,
                identity.instance_id,
                WorkflowInstanceAction::Pause,
                13,
            )
            .unwrap();
        checkpoint(&cut, "pause-requested");
    }
    if cut.starts_with("restart-") {
        controller
            .send_event(
                account,
                definition,
                identity.instance_id,
                "approval",
                "true",
                13,
            )
            .unwrap();
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Restart,
                &limits,
                14,
            )
            .unwrap();
        checkpoint(&cut, "restart-prepared");
        let WorkflowOperationResult::Applied(proof) = scheduler
            .apply_workflow_operation(&operation, 15, &limits)
            .unwrap()
        else {
            panic!("restart")
        };
        checkpoint(&cut, "restart-applied");
        repo.complete_instance_operation(&proof, 16).unwrap();
        checkpoint(&cut, "restart-finalized");
    }
    if cut.starts_with("purge-") || cut == "terminal-unretained" {
        scheduler
            .modify_workflow_v2(&identity, WorkflowInstanceAction::Terminate, 20, &limits)
            .unwrap();
        checkpoint(&cut, "terminal-unretained");
        repo.retain_instance(&identity, 20).unwrap();
        let expiry = 3600020;
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                &limits,
                expiry,
            )
            .unwrap();
        checkpoint(&cut, "purge-prepared");
        let WorkflowOperationResult::Applied(proof) = scheduler
            .apply_workflow_operation(&operation, expiry, &limits)
            .unwrap()
        else {
            panic!("purge")
        };
        checkpoint(&cut, "purge-deleted");
        repo.complete_instance_operation(&proof, expiry).unwrap();
        checkpoint(&cut, "purge-released");
        let receipt = scheduler.workflow_gc_receipts(None, 1).unwrap().remove(0);
        scheduler
            .sweep_workflow_gc(&repo.acknowledge_workflow_gc(&receipt).unwrap())
            .unwrap();
        checkpoint(&cut, "purge-swept");
    }
    scheduler
        .settle_workflow_step_v2(
            &run.fence,
            &attempt,
            WorkflowStepOutcome::Success("7"),
            13,
            &limits,
        )
        .unwrap();
    let wait = WorkflowStepDeclaration {
        ordinal: 1,
        name: "approval".into(),
        name_count: 1,
        kind: WorkflowStepKind::WaitEvent,
        config: serde_json::json!({"type":"approval","timeout":300000}),
        batch_first_ordinal: 1,
        batch_size: 1,
        dependencies: vec![0],
    }
    .resolve()
    .unwrap();
    scheduler
        .register_workflow_wait_v2(&run.fence, &wait, 14, &limits)
        .unwrap();
    checkpoint(&cut, "wait-registered");
    if cut == "event-committed" {
        controller
            .send_event(
                account,
                definition,
                identity.instance_id,
                "approval",
                "true",
                15,
            )
            .unwrap();
        checkpoint(&cut, "event-committed");
    }
    scheduler.yield_workflow_v2(&run.fence, 16).unwrap();
    checkpoint(&cut, "yield-committed");
    panic!("unrecognized V2 boundary");
}

#[test]
fn workflow_sigkill_durable_wait_retry_pause_restart_and_purge_boundaries() {
    for cut in [
        "attempt-granted",
        "retry-committed",
        "pause-requested",
        "wait-registered",
        "event-committed",
        "yield-committed",
        "restart-prepared",
        "restart-applied",
        "restart-finalized",
        "terminal-unretained",
        "purge-prepared",
        "purge-deleted",
        "purge-released",
        "purge-swept",
    ] {
        let (temp, storage, scheduler, definition) = durable_fixture();
        let root = storage.data_dir().root().to_owned();
        let _evidence = Evidence(Some(temp));
        drop(scheduler);
        drop(storage);
        kill_at_boundary(&root, definition, cut, DURABLE_CHILD);
        let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
        let now = if cut.starts_with("purge-") {
            3600021
        } else {
            120000
        };
        let scheduler =
            SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5000, now).unwrap();
        let repo = WorkflowRepository::new(storage.db());
        let account = storage.identity().default_account_id;
        let limits = WorkflowsConfig::default();
        let controller = WorkflowController::new(&storage, &scheduler, &limits);
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, now)
            .unwrap();
        if cut.starts_with("purge-") {
            assert_eq!(
                repo.find_instance(definition, "reusable")
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowInstanceNotFound
            );
            assert!(
                scheduler
                    .workflow_instance_ids(None, 32)
                    .unwrap()
                    .is_empty()
            );
            assert!(scheduler.workflow_gc_receipts(None, 32).unwrap().is_empty());
            create(&controller, account, definition, now + 1);
        } else {
            let reservation = repo.find_instance(definition, "reusable").unwrap();
            let record = scheduler
                .workflow_instance(reservation.identity.instance_id)
                .unwrap()
                .unwrap();
            assert_eq!(record.identity, reservation.identity, "{cut}");
            assert!(record.run_token.is_none(), "{cut}");
            assert_eq!(record.input_json, r#"{"value":7}"#);
            assert!(repo.instance_referrers_intact(&record.identity).unwrap());
            if cut.starts_with("restart-") {
                assert_eq!(record.identity.instance_generation, 2);
                assert_eq!(record.state, WorkflowState::Queued);
                assert_eq!(record.durable.as_ref().unwrap().registered_step_count, 0);
                assert_eq!(record.durable.as_ref().unwrap().event_count, 0);
            } else if cut == "pause-requested" {
                assert_eq!(record.state, WorkflowState::Paused);
                assert!(
                    controller
                        .claim(now, &mut Default::default())
                        .unwrap()
                        .is_none()
                );
            } else if cut == "terminal-unretained" {
                assert_eq!(record.state, WorkflowState::Terminated);
                assert_eq!(reservation.state, WorkflowRefState::Retained);
            } else if cut == "event-committed" {
                assert_eq!(record.state, WorkflowState::Queued);
                assert_eq!(record.completed_step_count, 2);
                assert_eq!(record.durable.as_ref().unwrap().event_count, 0);
            } else if matches!(cut, "wait-registered" | "yield-committed") {
                assert_eq!(record.state, WorkflowState::Waiting);
                assert_eq!(
                    record.durable.as_ref().unwrap().next_wake_at_ms,
                    Some(300014)
                );
                controller
                    .reconcile(&mut WorkflowReconcileCursor::default(), 32, 300014)
                    .unwrap();
                assert_eq!(
                    scheduler
                        .workflow_instance(record.identity.instance_id)
                        .unwrap()
                        .unwrap()
                        .state,
                    WorkflowState::Queued
                );
            } else {
                let admitted_at = now + i64::try_from(limits.recovery_backoff_ms).unwrap() + 200;
                let run = controller
                    .claim(admitted_at, &mut Default::default())
                    .unwrap()
                    .unwrap();
                let WorkflowV2StepGrant::Run { attempt, .. } = scheduler
                    .claim_workflow_batch_v2(
                        &run.fence,
                        &[descriptor()],
                        limits.dispatch_timeout_ms,
                        admitted_at,
                        &limits,
                    )
                    .unwrap()
                    .remove(0)
                else {
                    panic!("durable retry: {cut}")
                };
                assert_eq!(
                    attempt, 2,
                    "Unknown recovery must not itself consume an attempt"
                );
            }
            scheduler
                .verify_workflow_history(record.identity.instance_id)
                .unwrap();
        }
        repo.verify_catalog().unwrap();
        scheduler.quick_check().unwrap();
    }
}
