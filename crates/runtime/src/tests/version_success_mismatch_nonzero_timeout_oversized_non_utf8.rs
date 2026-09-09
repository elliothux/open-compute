use super::*;

#[tokio::test]
async fn version_success_mismatch_nonzero_timeout_oversized_non_utf8() {
    let dir = TempDir::new().unwrap();

    let ok = dir.path().join("ok");
    write_exec(&ok, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&ok));
    let verified = verify_ok(&lock_path, &ok).await;
    assert_eq!(verified.version_output(), VERSION);
    assert_eq!(verified, verified.clone());
    let verified_debug = format!("{verified:?}");
    assert!(verified_debug.contains(verified.target()));
    assert!(verified_debug.contains(verified.release()));
    assert!(verified_debug.contains(verified.binary_sha256()));

    let mismatch = dir.path().join("mismatch");
    write_exec(&mismatch, "#!/bin/sh\necho 'workerd 1999-01-01'\n");
    let mlock = write_lock(dir.path(), &sha256_file(&mismatch));
    assert_eq!(
        verify_runtime_binary(&mlock, &mismatch, Duration::from_secs(2), &Redactor::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    let nonzero = dir.path().join("nonzero");
    write_exec(&nonzero, &format!("#!/bin/sh\necho '{VERSION}'\nexit 3\n"));
    let nlock = dir.path().join("n.lock.json");
    fs::write(&nlock, lock_json(&sha256_file(&nonzero), "")).unwrap();
    assert!(
        verify_runtime_binary(&nlock, &nonzero, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );

    let sleepy = dir.path().join("sleep");
    write_exec(&sleepy, "#!/bin/sh\nsleep 30\n");
    let slock = dir.path().join("s.lock.json");
    fs::write(&slock, lock_json(&sha256_file(&sleepy), "")).unwrap();
    let started = std::time::Instant::now();
    let err = verify_runtime_binary(
        &slock,
        &sleepy,
        Duration::from_millis(400),
        &Redactor::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(started.elapsed() < Duration::from_secs(5));

    let big = dir.path().join("big");
    write_exec(
        &big,
        "#!/bin/sh\nawk 'BEGIN{for(i=0;i<9000;i++)printf \"a\"}'\n",
    );
    let block = dir.path().join("b.lock.json");
    fs::write(&block, lock_json(&sha256_file(&big), "")).unwrap();
    assert!(
        verify_runtime_binary(&block, &big, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );

    let binary = dir.path().join("binout");
    write_exec(&binary, "#!/bin/sh\nprintf '\\xff\\xfe'\n");
    let block2 = dir.path().join("b2.lock.json");
    fs::write(&block2, lock_json(&sha256_file(&binary), "")).unwrap();
    assert!(
        verify_runtime_binary(&block2, &binary, Duration::from_secs(2), &Redactor::new())
            .await
            .is_err()
    );
}
