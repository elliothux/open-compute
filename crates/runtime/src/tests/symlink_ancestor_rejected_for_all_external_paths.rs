use super::*;

#[tokio::test]
async fn symlink_ancestor_rejected_for_all_external_paths() {
    for relative in [false, true] {
        let fx = ancestor_fixture(relative, "workerd");
        write_exec(&fx.outside.join("workerd"), &version_script(None));
        let dummy = TempDir::new().unwrap();
        let bin_ok = dummy.path().join("workerd");
        write_exec(&bin_ok, &version_script(None));
        let lock_ok = write_lock(dummy.path(), &sha256_file(&bin_ok));
        let err = verify_runtime_binary(
            &lock_ok,
            &fx.linked_leaf,
            Duration::from_secs(2),
            &Redactor::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "workerd.lock.json");
        fs::write(
            fx.outside.join("workerd.lock.json"),
            lock_json(&"ab".repeat(32), ""),
        )
        .unwrap();
        let err = load_runtime_lock(&fx.linked_leaf).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "assets");
        fs::create_dir(fx.outside.join("assets")).unwrap();
        copy_formal_assets(&fx.outside.join("assets"));
        let dir = TempDir::new().unwrap();
        copy_formal_assets(dir.path());
        let bin = dir.path().join("workerd");
        write_exec(&bin, &version_script(None));
        let lock_path = write_lock(dir.path(), &sha256_file(&bin));
        let runtime = verify_ok(&lock_path, &bin).await;
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        let token = SecretString::new(TOKEN);
        let redactor = redactor_with_token();
        let platform = platform_meta();
        let err = compile_static_config(compile_req(
            &runtime,
            &lock_path,
            &fx.linked_leaf,
            &data,
            &platform,
            &token,
            &redactor,
            Duration::from_secs(5),
        ))
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "data");
        let err = compile_static_config(compile_req(
            &runtime,
            &lock_path,
            dir.path(),
            &fx.linked_leaf,
            &platform,
            &token,
            &redactor,
            Duration::from_secs(5),
        ))
        .await
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);

        let fx = ancestor_fixture(relative, "rel");
        let err = crate::materialize_embedded_runtime(&fx.linked_leaf).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathInvalid);
        assert_outside_untouched(&fx);
    }
}
