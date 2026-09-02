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
        config: open_compute_core::PlatformConfig::default(),
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
