use super::*;

#[test]
fn lock_target_url_identity_and_accessors_are_strict() {
    let good = lock_json(&"ab".repeat(32), "");
    let archive = host_archive();
    let url =
        format!("https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{archive}");
    let bad_urls = [
        "not a url".to_owned(),
        url.replacen("https://", "http://", 1),
        url.replacen("https://", "https://user:pass@", 1),
        url.replace("github.com", "example.com"),
        format!("{url}?download=1"),
        format!("{url}#fragment"),
        url.replace("v1.20260830.1", "v1.other"),
    ];
    for bad_url in bad_urls {
        let bad = good.replacen(&url, &bad_url, 1);
        assert!(
            RuntimeLock::parse(bad.as_bytes()).is_err(),
            "malformed archive URL unexpectedly accepted: {bad_url}"
        );
    }
    for bad_name in ["", "sub/workerd.gz", "sub\\workerd.gz"] {
        let bad = good.replacen(
            &format!("\"archiveName\": \"{archive}\""),
            &format!("\"archiveName\": \"{bad_name}\""),
            1,
        );
        assert!(RuntimeLock::parse(bad.as_bytes()).is_err());
    }
    let bad_archive_hash = good.replacen(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "zz",
        1,
    );
    assert!(RuntimeLock::parse(bad_archive_hash.as_bytes()).is_err());

    let lock = RuntimeLock::parse(good.as_bytes()).unwrap();
    let (name, target) = lock.current_target().unwrap();
    assert_eq!(name, host_target());
    let debug = format!("{target:?}");
    assert!(debug.contains(&target.archive_name));
    assert_eq!(
        RuntimeLock::token_placeholder(),
        "__OPEN_COMPUTE_INTERNAL_TOKEN__"
    );

    let dir = TempDir::new().unwrap();
    assert_eq!(
        load_runtime_lock(Path::new("relative.lock"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        load_runtime_lock(dir.path()).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}
