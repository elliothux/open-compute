use super::*;

#[test]
fn seeded_inconsistent_migrations_fail_closed() {
    for mutation in [
        "DELETE FROM refinery_schema_history WHERE version=1",
        "UPDATE refinery_schema_history SET name='wrong' WHERE version=1",
        "UPDATE refinery_schema_history SET applied_on='invalid' WHERE version=1",
    ] {
        let (_temp, root) = unique_root();
        let config = storage_config(&root);
        drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
        let connection = Connection::open(root.join("control.sqlite")).unwrap();
        connection.execute(mutation, []).unwrap();
        drop(connection);
        assert_eq!(
            PlatformStorage::bootstrap(&config, &SystemClock)
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
    }
}
