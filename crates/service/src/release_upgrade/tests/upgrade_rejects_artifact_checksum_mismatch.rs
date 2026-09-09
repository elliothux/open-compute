use super::*;

#[tokio::test]
async fn upgrade_rejects_artifact_checksum_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.7");
    let tag = "v0.1.7";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let wrong_digest = "ab".repeat(32);
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.7",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": next.len(),
            "sha256": wrong_digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    // SUMS must match the (wrong) manifest digest so resolve succeeds; download then fails.
    let sums = format!(
        "{wrong_digest}  {filename}\n{}  release.json\n\n",
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let base = format!("{download_base}/{tag}");
    http.insert(
        format!("{api_base}/repos/elliothux/open-compute/releases/latest"),
        format!(r#"{{"tag_name":"{tag}","prerelease":false,"draft":false}}"#),
    );
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), sums.into_bytes());
    http.insert(format!("{base}/{filename}"), next);
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.7"),
        false,
        true,
        "0.1.0",
    );
    let err = run_upgrade(
        &options,
        &http,
        &InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        &FakeServiceManager::default(),
        &mut Vec::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
}
