use super::*;

struct VisitorExpectation;

impl std::fmt::Display for VisitorExpectation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        NoDupVisitor.expecting(formatter)
    }
}

#[test]
fn private_lock_helpers_cover_unavailable_target_and_visitor_contracts() {
    let lock = RuntimeLock {
        schema_version: SCHEMA_VERSION,
        release: "v1.20260826.1".to_owned(),
        expected_version_output: "workerd 2026-08-26".to_owned(),
        host_compatibility_date: "2026-08-22".to_owned(),
        process_flags: vec!["--experimental".to_owned()],
        host_compatibility_flags: vec!["rpc".to_owned()],
        targets: BTreeMap::new(),
    };
    assert_eq!(
        lock.current_target().unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    let target = RuntimeTarget {
        archive_name: "workerd-unknown.gz".to_owned(),
        archive_url:
            "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/workerd-unknown.gz"
                .to_owned(),
        archive_sha256: "aa".repeat(32),
        binary_sha256: "bb".repeat(32),
    };
    assert_eq!(
        target
            .validate("v1.20260826.1", "unknown-target")
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    assert_eq!(
        VisitorExpectation.to_string(),
        "a JSON value without duplicate object keys"
    );
    let null = NoDupVisitor.visit_none::<de::value::Error>().unwrap();
    assert_eq!(null.0, serde_json::Value::Null);
}
