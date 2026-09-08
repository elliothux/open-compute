use super::*;

#[test]
fn version_pipeline_helper_contracts_cover_failure_code_matrix() {
    for key in ["", "contains space", "line\nbreak", &"x".repeat(129)] {
        assert_eq!(
            validate_idempotency_key(key).unwrap_err().code(),
            ErrorCode::IdempotencyConflict
        );
    }
    validate_idempotency_key("valid-key_123").unwrap();

    let account = AccountId::generate();
    let first = idempotency_ref_id(account, "version.create", "key");
    assert_eq!(first.len(), 64);
    assert_eq!(first, idempotency_ref_id(account, "version.create", "key"));
    assert_ne!(
        first,
        idempotency_ref_id(account, "version.create", "other")
    );

    let mut secrets = BTreeMap::new();
    assert!(validate_secret_set(&secrets, &BTreeMap::new()).is_ok());
    for index in 0..129 {
        secrets.insert(format!("SECRET_{index}"), SecretString::new("x"));
    }
    assert_eq!(
        validate_secret_set(&secrets, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretInvalid
    );
    for (name, value) in [
        ("BAD-NAME", "x".to_owned()),
        ("EMPTY", String::new()),
        ("LARGE", "x".repeat(MAX_VARIABLE_BYTES + 1)),
    ] {
        let values = BTreeMap::from([(name.to_owned(), SecretString::new(value))]);
        assert!(validate_secret_set(&values, &BTreeMap::new()).is_err());
    }
    let conflict = BTreeMap::from([("TOKEN".to_owned(), SecretString::new("x"))]);
    assert_eq!(
        validate_secret_set(
            &conflict,
            &BTreeMap::from([("TOKEN".to_owned(), serde_json::json!(1))]),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretInvalid
    );

    for (input, expected) in [
        (ErrorCode::RuntimeUnavailable, ErrorCode::RuntimeUnavailable),
        (
            ErrorCode::RuntimeResultUnknown,
            ErrorCode::RuntimeResultUnknown,
        ),
        (
            ErrorCode::ResourceLimitExceeded,
            ErrorCode::ResourceLimitExceeded,
        ),
        (ErrorCode::ConfigInvalid, ErrorCode::BundleRuntimeInvalid),
    ] {
        assert_eq!(
            stable_validation_code(&open_compute_core::PlatformError::new(input, "safe")),
            expected
        );
    }
    let failure_codes = [
        ("ACCOUNT_NOT_FOUND", ErrorCode::AccountNotFound),
        ("WORKER_NOT_FOUND", ErrorCode::WorkerNotFound),
        ("WORKER_DELETED", ErrorCode::WorkerDeleted),
        ("VERSION_NOT_FOUND", ErrorCode::VersionNotFound),
        ("VERSION_NOT_READY", ErrorCode::VersionNotReady),
        (
            "VERSION_INVARIANT_VIOLATION",
            ErrorCode::VersionInvariantViolation,
        ),
        ("BUNDLE_INVALID", ErrorCode::BundleInvalid),
        ("BUNDLE_TOO_LARGE", ErrorCode::BundleTooLarge),
        ("BUNDLE_RUNTIME_INVALID", ErrorCode::BundleRuntimeInvalid),
        (
            "COMPATIBILITY_UNSUPPORTED",
            ErrorCode::CompatibilityUnsupported,
        ),
        ("ARTIFACT_UNAVAILABLE", ErrorCode::ArtifactUnavailable),
        (
            "ARTIFACT_INTEGRITY_ERROR",
            ErrorCode::ArtifactIntegrityError,
        ),
        ("SECRET_INVALID", ErrorCode::SecretInvalid),
        ("RESOURCE_LIMIT_EXCEEDED", ErrorCode::ResourceLimitExceeded),
        ("RUNTIME_UNAVAILABLE", ErrorCode::RuntimeUnavailable),
        ("RUNTIME_RESULT_UNKNOWN", ErrorCode::RuntimeResultUnknown),
        ("UNKNOWN", ErrorCode::Internal),
    ];
    for (code, expected) in failure_codes {
        assert_eq!(
            ErrorCode::from_stable_str(code).unwrap_or(ErrorCode::Internal),
            expected
        );
    }
}
