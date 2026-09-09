use super::*;

#[tokio::test]
async fn resolve_release_rejects_manifest_and_sums_corruption() {
    let http = FixtureReleaseHttp::default();
    let api = "https://fixture.test/api";
    let download = "https://fixture.test/download";
    let target = host_target();
    let tag = "v0.3.0";
    let base = format!("{download}/{tag}");
    let filename = format!("ocd-{tag}-{target}");

    // Tag/version mismatch between path and release.json.
    let mut manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": "v0.3.1",
        "version": "0.3.1",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": 4,
            "sha256": "ab".repeat(32),
        }]
    });
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("tag/version")
    );

    // Unsupported schema.
    manifest["schemaVersion"] = serde_json::json!(99);
    manifest["tag"] = serde_json::json!(tag);
    manifest["version"] = serde_json::json!("0.3.0");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("schema")
    );

    // Unstable version inside an otherwise well-formed schema-1 document.
    manifest["schemaVersion"] = serde_json::json!(1);
    manifest["version"] = serde_json::json!("0.3.0-rc.1");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(format!("{base}/SHA256SUMS"), b"aa  release.json\n");
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .code()
            == ErrorCode::ReleaseUnsupported
    );

    // Non-UTF8 SHA256SUMS.
    let good = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.3.0",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": 4,
            "sha256": "ab".repeat(32),
        }]
    });
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(format!("{base}/SHA256SUMS"), vec![0xff, 0xfe, 0xfd]);
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("UTF-8")
    );

    // Malformed SUMS lines and missing release.json entry.
    for sums in [
        b"onlydigest\n".as_slice(),
        b"abcd name extra\n".as_slice(),
        format!("{}  {filename}\n", "ab".repeat(32))
            .into_bytes()
            .as_slice(),
    ] {
        http.insert(format!("{base}/SHA256SUMS"), sums.to_vec());
        let _ = resolve_release(&http, api, download, Some("0.3.0"), target).await;
    }

    // Missing target artifact.
    let no_artifact = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.3.0",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": []
    });
    let bytes = serde_json::to_vec(&no_artifact).unwrap();
    let sums = format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes)));
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("artifact")
    );

    // release.json digest mismatch against SHA256SUMS.
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes);
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!(
            "{}  release.json\n{}  {filename}\n",
            "00".repeat(32),
            "ab".repeat(32)
        )
        .into_bytes(),
    );
    assert_eq!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    // Artifact listed in release.json but absent from SHA256SUMS.
    let bytes = serde_json::to_vec(&good).unwrap();
    http.insert(format!("{base}/release.json"), bytes.clone());
    http.insert(
        format!("{base}/SHA256SUMS"),
        format!("{}  release.json\n", hex::encode(Sha256::digest(&bytes))).into_bytes(),
    );
    assert!(
        resolve_release(&http, api, download, Some("0.3.0"), target)
            .await
            .unwrap_err()
            .message()
            .contains("SHA256SUMS")
    );
}
