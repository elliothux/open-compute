use super::*;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

fn fixture() -> (TempDir, TargetRegistry, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("targets");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let token = temp.path().join("deployer.token");
    fs::write(&token, "secret-value\n").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let registry = TargetRegistry::at(root.join("targets.toml"));
    (temp, registry, token)
}

fn add(registry: &TargetRegistry, token: &Path) -> TargetRecord {
    registry
        .add(
            "company-prod".parse().unwrap(),
            "https://compute.example/client/v4".parse().unwrap(),
            "0123456789abcdef0123456789abcdef".parse().unwrap(),
            token.to_path_buf(),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10),
        )
        .unwrap()
}

#[test]
fn add_list_get_remove_preserve_external_token() {
    let (_temp, registry, token) = fixture();
    let added = add(&registry, &token);
    assert_eq!(added.created_at, 10_000);
    assert_eq!(registry.list().unwrap(), vec![added.clone()]);
    assert_eq!(registry.get(&added.name).unwrap(), added);
    registry.remove(&added.name).unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert!(token.exists());
}

#[test]
fn duplicate_name_and_authority_fail_closed() {
    let (_temp, registry, token) = fixture();
    add(&registry, &token);
    let duplicate_name = registry.add(
        "company-prod".parse().unwrap(),
        "https://other.example/client/v4".parse().unwrap(),
        "1123456789abcdef0123456789abcdef".parse().unwrap(),
        token.clone(),
        SystemTime::now(),
    );
    assert_eq!(
        duplicate_name.unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );
    let duplicate_authority = registry.add(
        "other".parse().unwrap(),
        "https://compute.example/client/v4".parse().unwrap(),
        "0123456789abcdef0123456789abcdef".parse().unwrap(),
        token,
        SystemTime::now(),
    );
    assert_eq!(
        duplicate_authority.unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );
}

#[test]
fn token_and_registry_security_fail_closed() {
    let (temp, registry, token) = fixture();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        registry
            .add(
                "bad".parse().unwrap(),
                "https://compute.example/client/v4".parse().unwrap(),
                "0123456789abcdef0123456789abcdef".parse().unwrap(),
                token.clone(),
                SystemTime::now(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::TargetInvalid
    );
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    add(&registry, &token);
    fs::set_permissions(registry.path(), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        registry.list().unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );

    let link_path = temp.path().join("linked-targets.toml");
    symlink(registry.path(), &link_path).unwrap();
    let linked = TargetRegistry::at(link_path);
    assert_eq!(
        linked.list().unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );

    let token_link = temp.path().join("linked-token");
    symlink(&token, &token_link).unwrap();
    assert_eq!(
        registry
            .add(
                "linked-token".parse().unwrap(),
                "https://linked.example/client/v4".parse().unwrap(),
                "1123456789abcdef0123456789abcdef".parse().unwrap(),
                token_link,
                SystemTime::now(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::TargetInvalid
    );
}

#[test]
fn unknown_schema_and_fields_are_rejected() {
    let (_temp, registry, _token) = fixture();
    fs::write(registry.path(), "schema_version = 2\ntargets = []\n").unwrap();
    fs::set_permissions(registry.path(), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        registry.list().unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );
    fs::write(
        registry.path(),
        "schema_version = 1\nunknown = true\ntargets = []\n",
    )
    .unwrap();
    assert_eq!(
        registry.list().unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );
}

#[test]
fn registry_writes_are_owner_only_and_leave_no_staging_file() {
    let (_temp, registry, token) = fixture();
    add(&registry, &token);
    let meta = fs::metadata(registry.path()).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let parent = registry.path().parent().unwrap();
    assert!(
        fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp-"))
    );
}

#[test]
fn registry_directory_must_remain_owner_only() {
    let (_temp, registry, token) = fixture();
    add(&registry, &token);
    let parent = registry.path().parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        registry.list().unwrap_err().code(),
        ErrorCode::TargetRegistryInvalid
    );
}

#[test]
fn stale_atomic_staging_file_does_not_replace_committed_registry() {
    let (_temp, registry, token) = fixture();
    let record = add(&registry, &token);
    let stale = registry
        .path()
        .parent()
        .unwrap()
        .join(".tmp-interrupted-write");
    fs::write(&stale, b"not committed").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(registry.list().unwrap(), vec![record]);
    assert_eq!(fs::read(&stale).unwrap(), b"not committed");
}

#[test]
fn mutation_lock_symlink_is_rejected() {
    let (temp, registry, token) = fixture();
    let lock_target = temp.path().join("lock-target");
    fs::write(&lock_target, b"").unwrap();
    fs::set_permissions(&lock_target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(
        &lock_target,
        registry.path().parent().unwrap().join(".targets.lock"),
    )
    .unwrap();

    assert_eq!(
        registry
            .add(
                "blocked".parse().unwrap(),
                "https://blocked.example/client/v4".parse().unwrap(),
                "0123456789abcdef0123456789abcdef".parse().unwrap(),
                token,
                SystemTime::now(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::TargetRegistryInvalid
    );
}

#[test]
fn token_content_must_be_one_visible_ascii_value() {
    let (_temp, registry, token) = fixture();
    for value in ["\n", "leading space", "two\nlines\n", "非 ascii\n"] {
        fs::write(&token, value).unwrap();
        assert_eq!(
            registry
                .add(
                    "invalid-token".parse().unwrap(),
                    "https://invalid.example/client/v4".parse().unwrap(),
                    "0123456789abcdef0123456789abcdef".parse().unwrap(),
                    token.clone(),
                    SystemTime::now(),
                )
                .unwrap_err()
                .code(),
            ErrorCode::TargetInvalid
        );
    }
}
