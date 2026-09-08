use super::*;

#[tokio::test]
async fn upgrade_rejects_equal_version_and_staged_version_mismatch() {
    let temp = TempDir::new().unwrap();
    let (binary_path, receipt_path, _) = write_upgradeable_pair(&temp, "0.1.0");
    let download_base = "https://fixture.test/download".to_owned();
    let api_base = "https://fixture.test/api".to_owned();
    let http = FixtureReleaseHttp::default();
    let next = fake_binary("0.1.0");
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.0",
        host_target(),
        &next,
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path.clone(),
        Some("0.1.0"),
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
    assert!(err.message().contains("already installed"));

    // Staged binary --version does not contain the release identity.
    let mismatched = b"#!/bin/sh\necho \"ocd 9.9.9\"\n".to_vec();
    fixture_release(
        &http,
        &download_base,
        &api_base,
        "0.1.8",
        host_target(),
        &mismatched,
    );
    let options = base_options(
        &temp,
        &binary_path,
        receipt_path,
        Some("0.1.8"),
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
    assert!(
        err.message().contains("version") || err.code() == ErrorCode::ReleaseUnsupported,
        "{err:?}"
    );
}
