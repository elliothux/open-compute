use super::*;

#[test]
fn private_failure_and_process_group_helpers_are_fail_closed() {
    let failure = SpawnFailure::without_child(PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "test failure",
    ));
    assert_eq!(failure.error.code(), ErrorCode::RuntimeInvalid);
    assert!(failure.pid.is_none());
    assert!(failure.pgid.is_none());
    assert!(failure.completion.is_none());

    assert_eq!(read_pgid(0).unwrap_err().code(), ErrorCode::RuntimeInvalid);
    assert_eq!(
        read_pgid(i32::MAX).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    set_spawn_fail_point("unknown");
    assert!(fail_point() == FailPoint::None);
    let _ = last_spawned_pid();
}

#[test]
fn workerd_child_roots_follow_only_its_owned_instance_lease() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    create_dir_secure(&data).unwrap();
    create_dir_secure(&data.join("runtime")).unwrap();
    let mut cmd = std::process::Command::new("/bin/true");
    configure_private_roots(&mut cmd, &data.join("runtime/child.lease")).unwrap();
    assert_eq!(cmd.get_current_dir(), Some(data.as_path()));
    let env = cmd
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (key, expected) in [
        ("HOME", data.clone()),
        ("XDG_CACHE_HOME", data.join("cache")),
        ("TMPDIR", data.join("tmp")),
        ("TMP", data.join("tmp")),
        ("TEMP", data.join("tmp")),
    ] {
        assert_eq!(
            env.get(std::ffi::OsStr::new(key))
                .and_then(Option::as_deref),
            Some(expected.as_os_str())
        );
    }
    assert!(data.join("tmp").is_dir());
    assert_eq!(
        configure_private_roots(&mut cmd, &data.join("other/child.lease"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let linked = temp.path().join("linked");
    std::os::unix::fs::symlink(&data, &linked).unwrap();
    assert_eq!(
        configure_private_roots(&mut cmd, &linked.join("runtime/child.lease"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
