use super::*;

#[test]
fn cron_slots_retries_and_unknown_recovery_preserve_logical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let activation_id = CronActivationId::generate();
    let projection = CronScheduleProjection {
        activation_id,
        instance_id: store.instance_id(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        execution_generation: 1,
        activation_generation: 1,
        expression: "* * * * *".to_owned(),
        expression_sha256: [9; 32],
        parser_version: 1,
        next_fire_at_ms: 60_000,
        updated_at_ms: 1,
    };
    store.ensure_cron_schedule_projection(&projection).unwrap();
    store.activate_cron_schedule(activation_id, 1, 2).unwrap();
    assert_eq!(
        store
            .project_due_cron_slots(60_000, 300_000, 10)
            .unwrap()
            .projected,
        1
    );
    assert_eq!(
        store
            .project_due_cron_slots(60_000, 300_000, 10)
            .unwrap()
            .projected,
        0
    );
    let [first] = store
        .claim_cron_runs(60_000, 100, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(first.scheduled_at_ms, 60_000);
    assert_eq!(first.attempt, 1);
    assert_eq!(first.dispatch_deadline_at_ms, 960_000);
    assert_eq!(
        store
            .complete_cron_run(
                &first,
                CronCompletion::Failure {
                    no_retry: false,
                    error_code: "CRON_RUNTIME_EXCEPTION",
                },
                60_001,
                3,
            )
            .unwrap(),
        CronCompletionResult::Retried
    );
    let retry_at = store
        .cron_workload_summary(i64::MAX)
        .unwrap()
        .oldest_due_at_ms
        .unwrap();
    let [retry] = store
        .claim_cron_runs(retry_at, 10, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(retry.id, first.id);
    assert_eq!(retry.attempt, 2);
    assert_eq!(
        store
            .recover_expired_cron_runs(retry_at + 10, 5, 3, 1)
            .unwrap(),
        1
    );
    let [recovered] = store
        .claim_cron_runs(retry_at + 15, 10, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(recovered.id, first.id);
    assert_eq!(recovered.attempt, 3);
    assert_ne!(recovered.claim_token, retry.claim_token);
    assert_eq!(
        store
            .complete_cron_run(&recovered, CronCompletion::Success, retry_at + 16, 3)
            .unwrap(),
        CronCompletionResult::Terminal
    );
    assert_eq!(
        store
            .complete_cron_run(&retry, CronCompletion::Success, retry_at + 17, 3)
            .unwrap(),
        CronCompletionResult::Stale
    );
    assert_eq!(store.gc_cron_history(retry_at + 18, 1, 100).unwrap(), 1);

    // A permanently unknown delivery consumes its bounded attempts and remains terminal.
    store.project_due_cron_slots(120_000, 300_000, 10).unwrap();
    let [unknown] = store
        .claim_cron_runs(120_000, 10, 5, 1, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert!(
        store
            .mark_cron_unknown(&unknown, CronUnknownReason::ConnectionLoss)
            .unwrap()
    );
    assert_eq!(
        store.recover_expired_cron_runs(120_010, 5, 1, 10).unwrap(),
        1
    );
    let [unknown_retry] = store
        .claim_cron_runs(120_015, 10, 5, 1, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(unknown_retry.attempt, 2);
    assert_eq!(
        store.recover_expired_cron_runs(120_025, 5, 1, 10).unwrap(),
        1
    );
    let inspection = store
        .inspect_cron_runtime(activation_id, 1, 120_025)
        .unwrap();
    assert_eq!(inspection.last_outcome.as_deref(), Some("failed"));
    assert_eq!(
        inspection.last_error_code.as_deref(),
        Some("CRON_RETRY_EXHAUSTED")
    );
    assert_eq!(
        inspection.last_unknown_reason.as_deref(),
        Some("connection-loss")
    );

    // Drain terminalizes ready work immediately and prevents an expired claim from requeueing.
    store.project_due_cron_slots(180_000, 300_000, 10).unwrap();
    let [draining] = store
        .claim_cron_runs(180_000, 10, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        store
            .drain_cron_schedule(activation_id, 1, 180_001)
            .unwrap(),
        1
    );
    assert_eq!(
        store.recover_expired_cron_runs(180_010, 5, 3, 10).unwrap(),
        1
    );
    assert_eq!(
        store.cron_activation_in_flight(activation_id, 1).unwrap(),
        0
    );
    assert_eq!(
        store
            .inspect_cron_runtime(activation_id, 1, 180_010)
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("CRON_ACTIVATION_DRAINED")
    );
    assert_eq!(
        store
            .complete_cron_run(&draining, CronCompletion::Success, 180_011, 3)
            .unwrap(),
        CronCompletionResult::Stale
    );

    // A recovered delivery whose backoff reaches the fixed deadline is terminalized.
    let deadline_activation = CronActivationId::generate();
    store
        .ensure_cron_schedule_projection(&CronScheduleProjection {
            activation_id: deadline_activation,
            next_fire_at_ms: 240_000,
            updated_at_ms: 239_999,
            ..projection
        })
        .unwrap();
    store
        .activate_cron_schedule(deadline_activation, 1, 240_000)
        .unwrap();
    store.project_due_cron_slots(240_000, 300_000, 10).unwrap();
    let [deadline] = store
        .claim_cron_runs(240_000, 10, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        store.recover_expired_cron_runs(240_010, 5, 3, 10).unwrap(),
        1
    );
    let (runs, settled) = store
        .claim_cron_runs(deadline.dispatch_deadline_at_ms, 10, 5, 3, 10)
        .unwrap();
    assert!(runs.is_empty());
    assert_eq!(settled, 1);
    assert_eq!(
        store
            .inspect_cron_runtime(deadline_activation, 1, deadline.dispatch_deadline_at_ms)
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("CRON_DISPATCH_DEADLINE")
    );

    // A known failure cannot schedule its retry beyond the same fixed deadline.
    store.project_due_cron_slots(300_000, 300_000, 10).unwrap();
    let [late_failure] = store
        .claim_cron_runs(300_000, 10, 5, 3, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        store
            .complete_cron_run(
                &late_failure,
                CronCompletion::Failure {
                    no_retry: false,
                    error_code: "CRON_RUNTIME_EXCEPTION",
                },
                late_failure.dispatch_deadline_at_ms - 1,
                3,
            )
            .unwrap(),
        CronCompletionResult::Terminal
    );
    assert_eq!(
        store
            .inspect_cron_runtime(deadline_activation, 1, late_failure.dispatch_deadline_at_ms)
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("CRON_DISPATCH_DEADLINE")
    );
}
