use super::*;

#[tokio::test]
async fn missing_directory_rejected_before_version() {
    let dir = TempDir::new().unwrap();
    let counter = dir.path().join("ran");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &version_script(Some(&counter)));
    let hash = sha256_file(&bin);
    let lock_path = write_lock(dir.path(), &hash);

    let missing = dir.path().join("nope");
    let err = verify_runtime_binary(
        &lock_path,
        &missing,
        Duration::from_secs(2),
        &Redactor::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(!counter.exists());

    let as_dir = dir.path().join("workerd-dir");
    fs::create_dir(&as_dir).unwrap();
    assert!(
        verify_runtime_binary(
            &lock_path,
            &as_dir,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .is_err()
    );
    assert!(!counter.exists());
}
