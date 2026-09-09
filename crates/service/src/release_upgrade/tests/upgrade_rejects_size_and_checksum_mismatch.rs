use super::*;

#[tokio::test]
async fn upgrade_rejects_size_and_checksum_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.6");
    let tag = "v0.1.6";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let digest = hex::encode(Sha256::digest(&next));
    let (os, arch) = match target {
        "darwin-arm64" => ("darwin", "arm64"),
        "linux-x64" => ("linux", "x64"),
        "linux-arm64" => ("linux", "arm64"),
        _ => ("darwin", "arm64"),
    };
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.6",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": os,
            "arch": arch,
            "filename": filename,
            "bytes": next.len() + 1,
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let sums = format!(
        "{digest}  {filename}\n{}  release.json\n",
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
        Some("0.1.6"),
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
