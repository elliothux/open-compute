use super::*;

#[test]
fn config_path_rejections_do_not_echo_secrets() {
    // Unix socket fixtures need a short path even under a deeply nested worktree.
    let dir = TempDir::new_in("/tmp").unwrap();
    let rel = parse_from(["ocd", "run", "--config", "relative.toml"]).unwrap();
    let err = load_platform_config(rel.config.as_ref().unwrap()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    assert!(!err.to_string().contains("AKIA"));

    fs::create_dir(dir.path().join("a")).unwrap();
    let config = write_config(dir.path(), "");
    let dotted = dir.path().join("a/../config.toml");
    assert_eq!(
        load_platform_config(&dotted).unwrap().path,
        fs::canonicalize(config).unwrap()
    );

    let real_parent = dir.path().join("real-config");
    fs::create_dir(&real_parent).unwrap();
    let nested = real_parent.join("open-compute.toml");
    fs::write(
        &nested,
        r#"
[data]
path = "state"
master_key_file = "state/keys/master.key"

[storage]
backend = "local"
path = "state/objects"
"#,
    )
    .unwrap();
    let parent_alias = dir.path().join("config-alias");
    std::os::unix::fs::symlink(&real_parent, &parent_alias).unwrap();
    let loaded = load_platform_config_from(
        Path::new("config-alias/open-compute.toml"),
        &fs::canonicalize(dir.path()).unwrap(),
    )
    .unwrap();
    assert_eq!(loaded.path, fs::canonicalize(&nested).unwrap());
    let canonical_parent = fs::canonicalize(&real_parent).unwrap();
    assert_eq!(loaded.config.data.path, canonical_parent.join("state"));
    assert_eq!(
        loaded.config.object_storage.as_local().unwrap().path,
        canonical_parent.join("state/objects")
    );

    let link = dir.path().join("link.toml");
    let target = dir.path().join("real.toml");
    fs::write(&target, "not toml secret=AKIA123").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err = load_platform_config(&link).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    assert!(!format!("{err:?}").contains("AKIA"));

    let fifo = dir.path().join("fifo.toml");
    let _ = Proc::new("mkfifo").arg(&fifo).status();
    if fifo.exists() {
        let err = load_platform_config(&fifo).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    }

    let socket = dir.path().join("socket.toml");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert_eq!(
        load_platform_config(&socket).unwrap_err().code(),
        ErrorCode::ConfigPathInvalid
    );
    drop(listener);
    let directory = dir.path().join("directory.toml");
    fs::create_dir(&directory).unwrap();
    assert_eq!(
        load_platform_config(&directory).unwrap_err().code(),
        ErrorCode::ConfigPathInvalid
    );

    let big = dir.path().join("big.toml");
    let mut f = File::create(&big).unwrap();
    let chunk = vec![b'a'; 1024];
    for _ in 0..(MAX_CONFIG_BYTES / 1024 + 2) {
        f.write_all(&chunk).unwrap();
    }
    drop(f);
    let err = load_platform_config(&big).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);

    let non_utf = dir.path().join("bad.toml");
    fs::write(&non_utf, [0xff, 0xfe]).unwrap();
    let err = load_platform_config(&non_utf).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);

    let unknown = dir.path().join("unknown.toml");
    let path = write_config(dir.path(), "");
    let mut toml = fs::read_to_string(&path).unwrap();
    toml.push_str("\nnope = 1\n");
    fs::write(&unknown, toml).unwrap();
    let err = load_platform_config(&unknown).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);

    let unreadable = dir.path().join("unreadable.toml");
    fs::write(&unreadable, "").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    let err = load_platform_config(&unreadable).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
    let _ = dotted;
}
