use super::*;

#[tokio::test]
async fn variable_admission_and_restart_preserve_the_complete_immutable_environment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "variables", RequestId::generate(), 1, 1000)
        .unwrap()
        .0;
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        Arc::new(AcceptAllValidator),
        BundleLimits::default(),
    );
    let mut request = version_request(account, worker.id, "environment", "private-value");
    request.vars = (0..MAX_VARIABLES - 1)
        .map(|index| {
            (
                format!("VAR_{index}"),
                serde_json::json!("x".repeat(MAX_VARIABLE_BYTES)),
            )
        })
        .collect();
    let CreateVersionOutcome::Applied(result) =
        controller.create_version(request.clone()).await.unwrap()
    else {
        panic!("first creation must not replay");
    };
    let key = loader_key(account, worker.id, result.version.id);
    let digest = hex::encode(result.version.worker_code_sha256);
    let mut overflow = request.clone();
    overflow
        .secrets
        .insert("OVERFLOW".into(), SecretString::new("x"));
    assert_eq!(
        controller
            .create_version(overflow)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SecretInvalid
    );
    request.vars.insert(
        "VAR_0".into(),
        serde_json::json!("x".repeat(MAX_VARIABLE_BYTES + 1)),
    );
    assert_eq!(
        controller.create_version(request).await.unwrap_err().code(),
        ErrorCode::ResourceLimitExceeded
    );
    assert_eq!(
        WorkerRepository::new(storage.db())
            .list_versions(account, worker.id)
            .unwrap()
            .len(),
        1
    );
    drop(controller);
    drop(storage);

    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let source = RuntimeSource::new(storage, artifacts, BundleLimits::default());
    let snapshot = source
        .resolve(&key, &digest, RuntimeScope::Runtime)
        .await
        .unwrap();
    assert_eq!(snapshot.vars.len() + snapshot.secrets.len(), MAX_VARIABLES);
    assert!(
        snapshot
            .vars
            .values()
            .all(|value| value.as_str().unwrap().len() == MAX_VARIABLE_BYTES)
    );
    assert_eq!(snapshot.secrets["API_TOKEN"].expose(), "private-value");

    // Raw persisted bytes must already be canonical; reads may not normalize
    // whitespace away even when the parsed value would match the descriptor.
    let conn = rusqlite::Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute_batch("DROP TRIGGER version_vars_update_guard;")
        .unwrap();
    let noncanonical = format!(" \"{}\" ", "x".repeat(MAX_VARIABLE_BYTES)).into_bytes();
    conn.execute(
        "UPDATE version_vars SET value_json=?1 WHERE version_id=?2 AND name='VAR_0'",
        rusqlite::params![noncanonical, result.version.id.to_string()],
    )
    .unwrap();
    assert_eq!(
        source
            .resolve(&key, &digest, RuntimeScope::Runtime)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
}
