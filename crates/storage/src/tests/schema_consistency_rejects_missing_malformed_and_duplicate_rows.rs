use super::*;

#[test]
fn schema_consistency_rejects_missing_malformed_and_duplicate_rows() {
    assert_eq!(
        inspect_schema_after_raw_sql("PRAGMA user_version = 1;"),
        ErrorCode::MigrationFailed
    );

    let registry = crate::migrations::migration_registry();
    let checksum_1 = hex::encode(registry[0].2);
    let checksum_2 = hex::encode(registry[1].2);
    let cases = [
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(2, X'{checksum_2}');\
             PRAGMA user_version = 1;"
        ),
        "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
         INSERT INTO schema_migrations VALUES(0, X'00');\
         PRAGMA user_version = 2;"
            .to_owned(),
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(1, X'{checksum_1}');\
             INSERT INTO schema_migrations VALUES(1, X'{checksum_1}');\
             PRAGMA user_version = 1;"
        ),
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(2, X'{checksum_2}');\
             PRAGMA user_version = 2;"
        ),
        "CREATE TABLE schema_migrations(version INTEGER); PRAGMA user_version = 1;".to_owned(),
        "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 TEXT);\
         INSERT INTO schema_migrations VALUES(1, 'not-a-blob');\
         PRAGMA user_version = 1;"
            .to_owned(),
    ];
    for sql in cases {
        assert_eq!(
            inspect_schema_after_raw_sql(&sql),
            ErrorCode::MigrationFailed,
            "{sql}"
        );
    }
}
