use super::*;

#[test]
fn reopening_current_schema_preserves_definition_identity_and_alarm_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    let namespace = ResourceId::generate();
    let alarm = projection(namespace, object(namespace, 9), "preserve-token-01", 50);
    store.upsert_alarm(&alarm, 10).unwrap();
    drop(store);
    let before = Connection::open(&path).unwrap();
    let schema_before: Vec<(String, String)> = {
        let mut statement = before
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type IN ('table', 'index') AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let row_before: (String, i64, String) = before
        .query_row(
            "SELECT kind, due_at_ms, row_token FROM scheduled_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let migration_before: (String, Vec<u8>) = before
        .query_row(
            "SELECT name, checksum_sha256 FROM scheduler_migrations WHERE version = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(before);

    drop(SchedulerStore::open(&path, 100, 20).unwrap());
    let after = Connection::open(&path).unwrap();
    let schema_after: Vec<(String, String)> = {
        let mut statement = after
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type IN ('table', 'index') AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(schema_after, schema_before);
    assert_eq!(
        after
            .query_row(
                "SELECT kind, due_at_ms, row_token FROM scheduled_jobs",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?
                )),
            )
            .unwrap(),
        row_before
    );
    assert_eq!(
        after
            .query_row(
                "SELECT name, checksum_sha256 FROM scheduler_migrations WHERE version = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap(),
        migration_before
    );
}
