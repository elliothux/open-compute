use super::*;

#[test]
fn refinery_history_is_current_and_future_schema_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    drop(store);
    let connection = Connection::open(&path).unwrap();
    let history: (i64, String) = connection
        .query_row(
            "SELECT version,name FROM refinery_schema_history ORDER BY version DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        history,
        (
            current_scheduler_schema_version(),
            "instance_identity".to_owned()
        )
    );
    connection
        .execute(
            "INSERT INTO refinery_schema_history(version,name,applied_on,checksum)
             VALUES(?1,'future','1970-01-01T00:00:00Z','0')",
            [current_scheduler_schema_version() + 1],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(
            &path,
            100,
            20,
            "019c0000000070008000000000000001".parse().unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::SchemaTooNew
    );
}
