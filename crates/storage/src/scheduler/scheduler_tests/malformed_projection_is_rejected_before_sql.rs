use super::*;

#[test]
fn malformed_projection_is_rejected_before_sql() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    let invalid_projection = projection(namespace, object(namespace, 1), "short", 0);
    assert_eq!(
        store
            .upsert_alarm(&invalid_projection, 1)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
}
