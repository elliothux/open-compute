use super::*;

#[tokio::test]
async fn missing_symlink_directory_non_executable_tampered_rejected_before_version() {
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

    let link = dir.path().join("workerd.link");
    symlink(&bin, &link).unwrap();
    assert!(
        verify_runtime_binary(&lock_path, &link, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );
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

    let non_exec = dir.path().join("workerd-ne");
    fs::copy(&bin, &non_exec).unwrap();
    let mut perms = fs::metadata(&non_exec).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&non_exec, perms).unwrap();
    let ne_hash = sha256_file(&non_exec);
    let ne_lock_path = dir.path().join("ne.lock.json");
    fs::write(&ne_lock_path, lock_json(&ne_hash, "")).unwrap();
    assert!(
        verify_runtime_binary(
            &ne_lock_path,
            &non_exec,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .is_err()
    );
    assert!(!counter.exists());

    let tampered = dir.path().join("workerd-bad");
    write_exec(
        &tampered,
        &format!("#!/bin/sh\nprintf x >> /dev/null\necho '{VERSION}'\n# tampered\n"),
    );
    assert_eq!(
        verify_runtime_binary(
            &lock_path,
            &tampered,
            Duration::from_secs(2),
            &Redactor::new()
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );
    assert!(!counter.exists(), "tampered binary must not be executed");
}
