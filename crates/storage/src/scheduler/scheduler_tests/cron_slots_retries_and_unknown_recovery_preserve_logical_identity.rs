use super::*;

#[test]
fn cron_slots_retries_and_unknown_recovery_preserve_logical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let activation_id = CronActivationId::generate();
    let projection = NewCronSchedule {
        activation_id,
        instance_id: store.instance_id(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        execution_generation: 1,

        expression: "* * * * *".to_owned(),
        expression_sha256: [9; 32],
        parser_version: 1,
        next_fire_at_ms: 60_000,
        updated_at_ms: 1,
    };
    store.insert_cron_schedule(&projection).unwrap();
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
        .claim_cron_runs(60_000, 3, 10)
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
        .claim_cron_runs(retry_at, 3, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(retry.id, first.id);
    assert_eq!(retry.attempt, 2);
    drop(store);
    let store = open_store(&temp, retry_at + 10);
    let [recovered] = store
        .claim_cron_runs(retry_at + 260, 3, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(recovered.id, first.id);
    assert_eq!(recovered.attempt, 3);
    assert_eq!(
        store
            .complete_cron_run(&recovered, CronCompletion::Success, retry_at + 261, 3)
            .unwrap(),
        CronCompletionResult::Terminal
    );
    assert_eq!(
        store
            .complete_cron_run(&retry, CronCompletion::Success, retry_at + 262, 3)
            .unwrap(),
        CronCompletionResult::Stale
    );
    assert_eq!(store.gc_cron_history(retry_at + 263, 1, 100).unwrap(), 1);

    // A permanently unknown delivery consumes its bounded attempts and remains terminal.
    store.project_due_cron_slots(120_000, 300_000, 10).unwrap();
    let [unknown] = store
        .claim_cron_runs(120_000, 1, 10)
        .unwrap()
        .try_into()
        .unwrap();
    // Removing a schedule leaves an unknown claimed run bounded by its own deadline.
    store
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM cron_schedules WHERE activation_id=?1",
            [activation_id],
        )
        .unwrap();
    assert!(
        store
            .mark_cron_unknown(&unknown, CronUnknownReason::ConnectionLoss, 120_010, 1)
            .unwrap()
    );

    let [unknown_retry] = store
        .claim_cron_runs(120_260, 1, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(unknown_retry.attempt, 2);
    store
        .mark_cron_unknown(
            &unknown_retry,
            CronUnknownReason::ConnectionLoss,
            120_270,
            1,
        )
        .unwrap();
    let inspection = store.inspect_cron_runtime(activation_id, 120_025).unwrap();
    assert_eq!(inspection.last_outcome, Some(CronRunState::Failed));
    assert_eq!(
        inspection.last_error_code.as_deref(),
        Some("CRON_RETRY_EXHAUSTED")
    );
    assert_eq!(
        inspection.last_unknown_reason.as_deref(),
        Some("connection-loss")
    );

    // A recovered delivery whose backoff reaches the fixed deadline is terminalized.
    let deadline_activation = CronActivationId::generate();
    store
        .insert_cron_schedule(&NewCronSchedule {
            activation_id: deadline_activation,
            next_fire_at_ms: 240_000,
            updated_at_ms: 239_999,
            ..projection
        })
        .unwrap();
    store.project_due_cron_slots(240_000, 300_000, 10).unwrap();
    let [deadline] = store
        .claim_cron_runs(240_000, 3, 10)
        .unwrap()
        .try_into()
        .unwrap();
    drop(store);
    let store = open_store(&temp, deadline.dispatch_deadline_at_ms);
    let runs = store
        .claim_cron_runs(deadline.dispatch_deadline_at_ms, 3, 10)
        .unwrap();
    assert!(runs.is_empty());
    assert_eq!(
        store
            .inspect_cron_runtime(deadline_activation, deadline.dispatch_deadline_at_ms)
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("CRON_DISPATCH_DEADLINE")
    );

    // A known failure cannot schedule its retry beyond the same fixed deadline.
    store.project_due_cron_slots(300_000, 300_000, 10).unwrap();
    let [late_failure] = store
        .claim_cron_runs(300_000, 3, 10)
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
            .inspect_cron_runtime(deadline_activation, late_failure.dispatch_deadline_at_ms)
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("CRON_DISPATCH_DEADLINE")
    );
}
