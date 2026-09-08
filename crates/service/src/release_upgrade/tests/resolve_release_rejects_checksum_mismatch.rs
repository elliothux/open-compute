use super::*;

#[tokio::test]
async fn resolve_release_rejects_checksum_mismatch() {
    let http = FixtureReleaseHttp::default();
    let download_base = "https://fixture.test/download";
    let api_base = "https://fixture.test/api";
    let binary = fake_binary("0.1.5");
    let tag = "v0.1.5";
    let target = host_target();
    let filename = format!("ocd-{tag}-{target}");
    let digest = hex::encode(Sha256::digest(&binary));
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "tag": tag,
        "version": "0.1.5",
        "gitRevision": "abc",
        "workerdRelease": "1.0.0",
        "workerdLockSha256": "cd".repeat(32),
        "artifacts": [{
            "target": target,
            "os": "darwin",
            "arch": "arm64",
            "filename": filename,
            "bytes": binary.len(),
            "sha256": digest,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let bad_sums = format!(
        "{}  {filename}\n{}  release.json\n",
        "00".repeat(32),
        hex::encode(Sha256::digest(&manifest_bytes))
    );
    let base = format!("{download_base}/{tag}");
    http.insert(format!("{base}/release.json"), manifest_bytes);
    http.insert(format!("{base}/SHA256SUMS"), bad_sums.into_bytes());
    let err = resolve_release(&http, api_base, download_base, Some("0.1.5"), target)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
}
