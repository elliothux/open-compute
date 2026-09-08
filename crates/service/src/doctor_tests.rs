use super::*;

struct RejectWrites;

impl Write for RejectWrites {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("rejected"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn report_helpers_cover_all_statuses_bounds_and_write_failures() {
    let report = DoctorReport {
        schema_version: 1,
        command: "doctor",
        result: "failed",
        checks: vec![
            ok("ok", "ok", Some("value".to_owned())),
            warning("warning", "warning", None),
            failed("failed", ErrorCode::ConfigInvalid, "failed", None),
            skipped("skipped", "skipped"),
        ],
    };
    assert!(report.failed());
    let mut human = Vec::new();
    report.write(&mut human, false).unwrap();
    let human = String::from_utf8(human).unwrap();
    for status in ["ok", "warning", "failed", "skipped"] {
        assert!(human.contains(status));
    }
    let mut json = Vec::new();
    report.write(&mut json, true).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json).unwrap()["result"],
        "failed"
    );
    assert_eq!(
        report.write(&mut RejectWrites, false).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        report.write(&mut RejectWrites, true).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(bound_value("aéz", 3), "aé");
    assert_eq!(bound_value("aéz", 2), "a");
}

#[test]
fn workflow_doctor_fails_closed_when_authority_is_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let loaded = LoadedConfig {
        path: root.path().join("open-compute.toml"),
        config: open_compute_core::PlatformConfig::local_test_config(),
    };
    let check = workflow::inspect(&loaded, &root.path().join("missing"));
    assert_eq!(check.name, "workflow_authority");
    assert_eq!(check.status, CheckStatus::Failed);
    assert!(check.code.is_some());
    assert_eq!(check.value, None);
}

#[test]
fn ai_provider_readiness_is_local_and_requires_resolvable_credentials() {
    assert_eq!(
        inspect_ai_provider_config(&AiConfig::default()).unwrap(),
        "providers=0 embedding_models=0 generation_models=0"
    );
    let mut config = AiConfig::default();
    let missing_secret = tempfile::tempdir().unwrap().path().join("missing-secret");
    config.providers.insert(
        "offline".to_owned(),
        open_compute_core::AiProviderConfig {
            base_url: "https://provider.invalid/v1".to_owned(),
            auth: AiAuthConfig::Bearer {
                secret: open_compute_core::SecretReference {
                    env: None,
                    file: Some(missing_secret),
                },
            },
        },
    );
    assert_eq!(
        inspect_ai_provider_config(&config).unwrap_err().code(),
        ErrorCode::SecretRefInvalid
    );
}

#[tokio::test]
async fn full_runtime_checks_require_exclusive_authority_and_skip_missing_remotes() {
    let temporary = tempfile::tempdir().unwrap();
    let data_dir = temporary.path().join("data");
    let mut config = open_compute_core::PlatformConfig::local_test_config();
    config.data.path = data_dir.clone();
    config.data.master_key_file = data_dir.join("keys/master.key");
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let loaded = LoadedConfig {
        path: temporary.path().join("open-compute.toml"),
        config,
    };

    let busy = inspect_data_root(&loaded.config.data).unwrap();
    assert!(!busy.holds_inspect_lock());
    let mut checks = Vec::new();
    runtime::run_full_extras(&mut checks, &loaded, &busy, None, None).await;
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, CheckStatus::Failed);
    assert_eq!(checks[0].code, Some(ErrorCode::DataDirInUse.as_str()));

    drop(busy);
    drop(storage);
    let available = inspect_data_root(&loaded.config.data).unwrap();
    assert!(available.holds_inspect_lock());
    checks.clear();
    runtime::run_full_extras(&mut checks, &loaded, &available, None, None).await;
    assert!(checks.iter().any(|check| {
        check.name == "object_storage_canary" && check.status == CheckStatus::Skipped
    }));
    assert!(
        checks
            .iter()
            .any(|check| { check.name == "r2_canary" && check.status == CheckStatus::Skipped })
    );
    assert!(
        checks
            .iter()
            .any(|check| { check.name == "local_fsync" && check.status == CheckStatus::Skipped })
    );
    assert!(
        checks
            .iter()
            .any(|check| { check.name == "runtime_cycle" && check.status == CheckStatus::Skipped })
    );
}

fn bootstrapped_loaded(
    temporary: &tempfile::TempDir,
) -> (LoadedConfig, open_compute_storage::PlatformStorage) {
    let data_dir = temporary.path().join("data");
    let mut config = open_compute_core::PlatformConfig::local_test_config();
    config.data.path = data_dir.clone();
    config.data.master_key_file = data_dir.join("keys/master.key");
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let loaded = LoadedConfig {
        path: temporary.path().join("open-compute.toml"),
        config,
    };
    (loaded, storage)
}

#[tokio::test]
async fn doctor_report_basic_covers_bootstrapped_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let (loaded, storage) = bootstrapped_loaded(&temporary);
    drop(storage);
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(report.command, "doctor");
    assert_eq!(report.schema_version, 1);
    assert!(report.checks.iter().any(|c| c.name == "config"));
    assert!(report.checks.iter().any(|c| c.name == "data_dir"));
    assert!(report.checks.iter().any(|c| c.name == "sqlite"));
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "object_storage_canary" && c.status == CheckStatus::Skipped)
    );
    let mut out = Vec::new();
    report.write(&mut out, true).unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&out).is_ok());
}

#[tokio::test]
async fn doctor_report_missing_data_dir_skips_authority_checks() {
    let temporary = tempfile::tempdir().unwrap();
    let mut config = open_compute_core::PlatformConfig::local_test_config();
    config.data.path = temporary.path().join("missing-data");
    config.data.master_key_file = config.data.path.join("keys/master.key");
    let loaded = LoadedConfig {
        path: temporary.path().join("open-compute.toml"),
        config,
    };
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert!(report.failed());
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "data_dir" && c.status == CheckStatus::Failed)
    );
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "sqlite" && c.status == CheckStatus::Skipped)
    );
}

#[tokio::test]
async fn doctor_report_full_skips_when_lock_held() {
    let temporary = tempfile::tempdir().unwrap();
    let (loaded, storage) = bootstrapped_loaded(&temporary);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    drop(storage);
    assert!(report.checks.iter().any(|c| {
        c.name == "object_storage_canary"
            && c.status == CheckStatus::Skipped
            && c.message.contains("exclusive lock")
    }));
}

#[tokio::test]
async fn doctor_report_marks_config_when_metrics_limits_invalid() {
    let temporary = tempfile::tempdir().unwrap();
    let (mut loaded, storage) = bootstrapped_loaded(&temporary);
    drop(storage);
    loaded.config.metrics.max_series = 1;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    let config = report
        .checks
        .iter()
        .find(|c| c.name == "config")
        .expect("config check");
    assert_eq!(config.status, CheckStatus::Failed);
    assert_eq!(config.code, Some(ErrorCode::LimitInvalid.as_str()));
}

#[tokio::test]
async fn doctor_report_flags_impossible_free_space_thresholds() {
    let temporary = tempfile::tempdir().unwrap();
    let (mut loaded, storage) = bootstrapped_loaded(&temporary);
    drop(storage);
    loaded.config.data.free_space_hard_bytes = u64::MAX;
    loaded.config.data.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert!(
        report.checks.iter().any(|c| {
            c.name == "free_space" && matches!(c.status, CheckStatus::Failed | CheckStatus::Warning)
        }),
        "{:?}",
        report
            .checks
            .iter()
            .filter(|c| c.name == "free_space")
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn doctor_report_full_with_available_lock_runs_extras() {
    let temporary = tempfile::tempdir().unwrap();
    let (loaded, storage) = bootstrapped_loaded(&temporary);
    drop(storage);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "object_storage_canary")
    );
    assert!(report.checks.iter().any(|c| c.name == "runtime_cycle"));
}
