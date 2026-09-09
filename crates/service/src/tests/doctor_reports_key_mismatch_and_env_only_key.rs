use super::*;

#[tokio::test]
async fn doctor_reports_key_mismatch_and_env_only_key() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let other = encode_master_key(&[7u8; 32]);
    write_mode(&loaded.config.data.master_key_file, &other, 0o600);
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "master_key").status, CheckStatus::Failed);
    assert_eq!(
        check(&report, "master_key").code,
        Some("MASTER_KEY_MISMATCH")
    );

    open_compute_storage::set_test_env("OC_TEST_MASTER_KEY_ONLY", &encode_master_key(&[9u8; 32]));
    let mut cfg = loaded.config.data.clone();
    cfg.master_key_env = Some("OC_TEST_MASTER_KEY_ONLY".into());
    cfg.master_key_file = dir.path().join("missing-master.key");
    open_compute_storage::inspect_master_key(&cfg).expect("env-only key is readable");
    open_compute_storage::clear_test_env();
}
