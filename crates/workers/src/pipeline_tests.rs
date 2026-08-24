use super::*;

#[tokio::test]
async fn default_entrypoint_validation_and_internal_error_are_stable() {
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_: ValidationCandidate| async { Ok(()) });
    let candidate = ValidationCandidate {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: [3; 32],
    };
    assert_eq!(
        validator
            .validate_entrypoint(candidate, "named".to_owned())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::EntrypointNotFound
    );
    assert_eq!(invariant().code(), ErrorCode::DeploymentInvariantViolation);
}
