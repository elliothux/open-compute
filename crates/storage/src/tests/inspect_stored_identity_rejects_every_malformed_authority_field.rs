use super::*;

#[test]
fn inspect_stored_identity_rejects_every_malformed_authority_field() {
    let cases = [
        (
            "DELETE FROM platform_meta WHERE key = 'platform_id'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('bad' AS BLOB) WHERE key = 'platform_id'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'created_at_ms'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('bad' AS BLOB) WHERE key = 'created_at_ms'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'master_key_id'",
            ErrorCode::MigrationFailed,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'artifact_schema_version'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('2' AS BLOB) WHERE key = 'artifact_schema_version'",
            ErrorCode::MigrationFailed,
        ),
        ("DELETE FROM accounts", ErrorCode::MigrationFailed),
        (
            "UPDATE accounts SET id = 'invalid' WHERE name = 'default'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "UPDATE platform_meta SET value = X'FF' WHERE key = 'platform_id'",
            ErrorCode::ConfigInvalid,
        ),
    ];
    for (sql, expected) in cases {
        assert_eq!(inspect_identity_after(sql), expected, "{sql}");
    }
}
