use super::*;

#[tokio::test]
async fn fail_after_stages_release_lock_and_ports() {
    let mock = open_compute_artifacts::MockS3::spawn("open-compute").await;
    let dir = TempDir::new().unwrap();
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, FIXTURE_S3_ACCESS_KEY_ID, 0o600);
    write_mode(&sk, FIXTURE_S3_SECRET_ACCESS_KEY, 0o600);
    let extra = format!(
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
request_timeout_ms = 2000
"#,
        mock.endpoint,
        ak.display(),
        sk.display()
    );
    let path = write_config(dir.path(), &extra);
    let loaded = load_fixture_platform_config(&path);
    for stage in [
        FailAfter::Config,
        FailAfter::Storage,
        FailAfter::RuntimeVerify,
        FailAfter::ObjectStorage,
        FailAfter::Cache,
        FailAfter::Compile,
        FailAfter::Listen,
    ] {
        let opts = RunOptions {
            fail_after: Some(stage),
            ..RunOptions::default()
        };
        let err = Box::pin(run_platform_with(loaded.clone(), opts.clone()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigInvalid);
        let stages = opts.stages.lock().unwrap().clone();
        let expected: &[&str] = match stage {
            FailAfter::Config => &["config"],
            FailAfter::Storage => &["config", "storage"],
            FailAfter::RuntimeVerify => &["config", "storage", "runtime_verify"],
            FailAfter::ObjectStorage => &["config", "storage", "runtime_verify", "object_storage"],
            FailAfter::Cache => &[
                "config",
                "storage",
                "runtime_verify",
                "object_storage",
                "cache",
            ],
            FailAfter::Compile => &[
                "config",
                "storage",
                "runtime_verify",
                "object_storage",
                "cache",
                "compile",
            ],
            FailAfter::Listen => &[
                "config",
                "storage",
                "runtime_verify",
                "object_storage",
                "cache",
                "compile",
                "listen",
            ],
        };
        assert_eq!(stages, expected, "fail point {stage:?}");
        let addr = *opts.last_public_addr.lock().unwrap();
        if let Some(addr) = addr {
            let _rebind = tokio::net::TcpListener::bind(addr).await.expect("rebind");
        }
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .expect("lock reacquired");
        let expected_objects = usize::from(matches!(
            stage,
            FailAfter::ObjectStorage | FailAfter::Cache | FailAfter::Compile | FailAfter::Listen
        ));
        assert_eq!(mock.object_count(), expected_objects);
    }
}
