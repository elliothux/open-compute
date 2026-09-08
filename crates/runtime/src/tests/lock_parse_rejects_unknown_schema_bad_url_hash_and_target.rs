use super::*;

#[test]
fn lock_parse_rejects_unknown_schema_bad_url_hash_and_target() {
    let good = lock_json(&"ab".repeat(32), "");
    RuntimeLock::parse(good.as_bytes()).expect("good lock");

    let unknown = good.replace("\"schemaVersion\": 2", "\"schemaVersion\": 1");
    let err = RuntimeLock::parse(unknown.as_bytes()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);

    let extra_field = good.replacen('{', "{\"nope\":1,", 1);
    assert!(RuntimeLock::parse(extra_field.as_bytes()).is_err());

    let obsolete_date = good.replacen(
        "\"effectiveCompatibilityDate\"",
        "\"hostCompatibilityDate\"",
        1,
    );
    assert!(
        RuntimeLock::parse(obsolete_date.as_bytes()).is_err(),
        "obsolete hostCompatibilityDate must be rejected"
    );
    let obsolete_flags = good.replacen(
        "\"systemCompatibilityFlags\"",
        "\"hostCompatibilityFlags\"",
        1,
    );
    assert!(
        RuntimeLock::parse(obsolete_flags.as_bytes()).is_err(),
        "obsolete hostCompatibilityFlags must be rejected"
    );

    let bad_url = good.replace("https://github.com/", "http://example.com/");
    assert!(RuntimeLock::parse(bad_url.as_bytes()).is_err());

    let bad_hash = lock_json("zzzz", "");
    assert!(RuntimeLock::parse(bad_hash.as_bytes()).is_err());

    let bad_target = lock_json(
        &"ab".repeat(32),
        r#",
    "solaris-sparc": {
      "archiveName": "x.gz",
      "archiveUrl": "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/x.gz",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "binarySha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }"#,
    );
    assert!(RuntimeLock::parse(bad_target.as_bytes()).is_err());

    let dup_keys = good.replacen(
        "\"schemaVersion\": 2",
        "\"schemaVersion\": 2, \"schemaVersion\": 2",
        1,
    );
    assert!(RuntimeLock::parse(dup_keys.as_bytes()).is_err());

    let bad_date = good.replace(
        "\"effectiveCompatibilityDate\": \"2026-08-30\"",
        "\"effectiveCompatibilityDate\": \"2026-02-30\"",
    );
    assert!(RuntimeLock::parse(bad_date.as_bytes()).is_err());

    let dup_flag = good.replace(
        "[\"--experimental\"]",
        "[\"--experimental\", \"--experimental\"]",
    );
    assert!(RuntimeLock::parse(dup_flag.as_bytes()).is_err());

    let archive = host_archive();
    let bad_archive = good.replacen(
        &format!("\"archiveName\": \"{archive}\""),
        "\"archiveName\": \"other.gz\"",
        1,
    );
    assert!(RuntimeLock::parse(bad_archive.as_bytes()).is_err());

    let foreign = if host_target() == "linux-x64" {
        "darwin-arm64"
    } else {
        "linux-x64"
    };
    let foreign_archive = archive_for_target(foreign);
    let host_named_foreign = good
        .replace(
            &format!("\"archiveName\": \"{archive}\""),
            &format!("\"archiveName\": \"{foreign_archive}\""),
        )
        .replace(
            &format!(
                "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{archive}"
            ),
            &format!(
                "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{foreign_archive}"
            ),
        );
    assert!(
        RuntimeLock::parse(host_named_foreign.as_bytes()).is_err(),
        "archiveName must be workerd-<target-key>.gz even when the URL uses that foreign name"
    );

    let extra_mismatch = format!(
        r#",
    "{foreign}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{archive}",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "binarySha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }}"#
    );
    assert!(
        RuntimeLock::parse(lock_json(&"ab".repeat(32), &extra_mismatch).as_bytes()).is_err(),
        "a second target must not reuse another target's official archive name"
    );
}
