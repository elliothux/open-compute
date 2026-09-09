use super::*;

#[test]
fn retry_and_discarding_transitions_keep_cross_database_ordering() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "retry-token-0001", 10),
            1,
        )
        .unwrap();
    let [first] = store.claim_due(10, 100, 1).unwrap().try_into().unwrap();
    assert!(
        store
            .finish_claim(
                &first,
                ClaimResult::Reschedule {
                    due_at_ms: 2_010,
                    retry_count: 1,
                    last_error_code: Some("DO_RUNTIME_EXCEPTION"),
                },
                11,
            )
            .unwrap()
    );
    let [second] = store.claim_due(2_010, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(second.retry_count, 1);
    assert!(
        store
            .finish_claim(
                &second,
                ClaimResult::MarkDiscarding {
                    last_error_code: "DO_RUNTIME_EXCEPTION",
                },
                2_011,
            )
            .unwrap()
    );
    let summary = store.summary(2_011).unwrap();
    assert_eq!(summary.discarding, 1);
    assert_eq!(summary.claimed, 0);
    assert!(store.finish_discarding(&second).unwrap());
    assert_eq!(store.summary(2_012).unwrap(), SchedulerSummary::default());
}
