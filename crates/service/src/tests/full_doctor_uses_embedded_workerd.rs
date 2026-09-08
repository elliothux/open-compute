use super::*;

#[tokio::test]
async fn full_doctor_uses_embedded_workerd() {
    let (dir, _path, mock) = initialized_doctor_fixture().await;
    let extra = r#"
[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 5000
drain_timeout_ms = 5000
kill_timeout_ms = 2000
"#;
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, FIXTURE_S3_ACCESS_KEY_ID, 0o600);
    write_mode(&sk, FIXTURE_S3_SECRET_ACCESS_KEY, 0o600);
    let s3 = format!(
        r#"
[storage]
backend = "s3"
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{}"
secret_access_key_file = "{}"
verify_tls = true
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000
"#,
        mock.endpoint,
        ak.display(),
        sk.display()
    );
    let path = write_config(dir.path(), &format!("{s3}\n{extra}"));
    let loaded = load_fixture_platform_config(&path);
    assert!(loaded.config.data.path.join("control.sqlite").exists());
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(
        check(&report, "runtime_cycle").status,
        CheckStatus::Ok,
        "{:?}",
        report.checks
    );
    assert_eq!(
        check(&report, "object_storage_canary").status,
        CheckStatus::Ok
    );
    assert_eq!(check(&report, "s3_tls").status, CheckStatus::Ok);
    assert_eq!(check(&report, "s3_connectivity").status, CheckStatus::Ok);
    assert_eq!(
        check(&report, "s3_provider_capability").status,
        CheckStatus::Ok
    );
    assert_eq!(mock.object_count(), 1);
}
