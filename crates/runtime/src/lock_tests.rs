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
        release: "v1.20260830.1".to_owned(),
        revision: "e9dda5963aba7ee4323960db795690ec78fec118".to_owned(),
        expected_version_output: "workerd 2026-08-30".to_owned(),
        effective_compatibility_date: "2026-08-30".to_owned(),
        required_compatibility_flags: Vec::new(),
        system_compatibility_flags: vec!["experimental".to_owned()],
        process_flags: vec!["--experimental".to_owned()],
        workers_types: WorkersTypesPin {
            version: "5.20260830.1".to_owned(),
            git_head: "e9dda5963aba7ee4323960db795690ec78fec118".to_owned(),
            package_sha256: "aa".repeat(32),
            ast_sha256: "bb".repeat(32),
        },
        workers_sdk: WorkersSdkPin {
            revision: "f8085545bcaa2c639f171c25e4424685036a0e10".to_owned(),
            wrangler_version: "4.127.1".to_owned(),
            vite_plugin_version: "1.54.2".to_owned(),
        },
        targets: BTreeMap::new(),
    };
    assert_eq!(
        lock.current_target().unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    let target = RuntimeTarget {
        archive_name: "workerd-unknown.gz".to_owned(),
        archive_url:
            "https://github.com/cloudflare/workerd/releases/download/v1.20260830.1/workerd-unknown.gz"
                .to_owned(),
        archive_sha256: "aa".repeat(32),
        binary_sha256: "bb".repeat(32),
    };
    assert_eq!(
        target
            .validate("v1.20260830.1", "unknown-target")
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
