use super::*;

#[test]
fn inspect_stored_identity_rejects_every_malformed_authority_field() {
    let cases = [
        ("DELETE FROM instance_identity", ErrorCode::MigrationFailed),
        (
            "UPDATE instance_identity SET instance_id = 'bad'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "UPDATE instance_identity SET created_at_ms = -1",
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
    ];
    for (sql, expected) in cases {
        assert_eq!(inspect_identity_after(sql), expected, "{sql}");
    }
}
