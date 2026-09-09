use super::*;

#[test]
fn cron_slots_retries_and_unknown_recovery_preserve_logical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let activation_id = CronActivationId::generate();
    let projection = CronScheduleProjection {
        activation_id,
        account_id: AccountId::generate(),
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
        .claim_cron_runs(60_000, 100, 5, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(first.scheduled_at_ms, 60_000);
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
        .claim_cron_runs(retry_at, 10, 5, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(retry.id, first.id);
    assert_eq!(retry.attempt, 1);
    assert_eq!(
        store
            .recover_expired_cron_runs(retry_at + 10, 5, 1)
            .unwrap(),
        1
    );
    let [recovered] = store
        .claim_cron_runs(retry_at + 15, 10, 5, 10)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(recovered.id, first.id);
    assert_eq!(recovered.attempt, 1);
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
}
