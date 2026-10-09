use super::*;

fn config() -> BrowserConfig {
    BrowserConfig {
        public_origin: None,
        max_sessions: 8,
        max_pending_acquires: 16,
        acquire_timeout_ms: 10_000,
        command_timeout_ms: 30_000,
        max_connections: 16,
        max_actions: 8,
        max_body_bytes: 1024 * 1024,
        max_download_bytes: 1024 * 1024,
        max_download_files: 16,
        max_result_bytes: 16 * 1024 * 1024,
        max_message_bytes: 1024 * 1024,
        max_queued_messages: 16,
        max_history_entries: 1000,
        history_retention_ms: 86_400_000,
        max_frontend_requests: 64,
        backend: BrowserBackendConfig::Managed {
            executable: "/opt/chrome-headless-shell/chrome-headless-shell".into(),
            browser_idle_timeout_ms: 30_000,
            shutdown_grace_ms: 3_000,
        },
    }
}

#[test]
fn browser_config_requires_explicit_bounded_limits_and_exclusive_backend() {
    let original = config();
    original.validate().unwrap();
    let value = serde_json::to_value(&original).unwrap();
    for (key, maximum) in [
        ("max_sessions", 1_024_u64),
        ("max_pending_acquires", 4_096),
        ("acquire_timeout_ms", 120_000),
        ("command_timeout_ms", 120_000),
        ("max_connections", 4_096),
        ("max_actions", 1_024),
        ("max_body_bytes", 16 * 1024 * 1024),
        ("max_download_bytes", 128 * 1024 * 1024),
        ("max_download_files", 4_096),
        ("max_result_bytes", 128 * 1024 * 1024),
        ("max_message_bytes", 16 * 1024 * 1024),
        ("max_queued_messages", 1_024),
        ("max_history_entries", 100_000),
        ("history_retention_ms", 31_536_000_000),
        ("max_frontend_requests", 1_024),
    ] {
        for invalid in [0, maximum + 1] {
            let mut candidate = value.clone();
            candidate[key] = invalid.into();
            let parsed: BrowserConfig = serde_json::from_value(candidate).unwrap();
            assert_eq!(
                parsed.validate().unwrap_err().code(),
                ErrorCode::LimitInvalid,
                "{key}"
            );
        }
        let mut candidate = value.clone();
        candidate.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<BrowserConfig>(candidate).is_err(),
            "{key}"
        );
    }
    for (field, invalid) in [
        ("executable", serde_json::json!("relative/chrome")),
        ("executable", serde_json::json!("/")),
        ("browser_idle_timeout_ms", serde_json::json!(0)),
        ("browser_idle_timeout_ms", serde_json::json!(3_600_001)),
        ("shutdown_grace_ms", serde_json::json!(0)),
        ("shutdown_grace_ms", serde_json::json!(30_001)),
    ] {
        let mut candidate = value.clone();
        candidate["backend"][field] = invalid;
        assert!(
            serde_json::from_value::<BrowserConfig>(candidate)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut candidate = value;
    candidate["backend"]["url"] = "http://localhost:9222".into();
    assert!(serde_json::from_value::<BrowserConfig>(candidate).is_err());
}

#[test]
fn browser_public_origin_is_explicit_and_rejects_credentials_and_non_origins() {
    let mut candidate = config();
    for origin in [
        "https://control.example:8443",
        "http://192.0.2.1:8787/",
        "https://[2001:db8::1]",
    ] {
        candidate.public_origin = Some(origin.into());
        candidate.validate().unwrap();
    }
    for origin in [
        "",
        "file:///tmp",
        "https://user:secret@example.com",
        "https://example.com/path",
        "https://example.com?token=secret",
        "https://example.com/#secret",
        "https://example.com\n",
        "https://example.com /",
        "//example.com",
    ] {
        candidate.public_origin = Some(origin.into());
        let error = candidate.validate().unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        assert!(!error.to_string().contains("secret"));
    }
}

#[test]
fn browser_cdp_endpoints_reject_credentials_and_page_scope_without_leaking_input() {
    for value in [
        "http://127.0.0.1:9222",
        "https://browser.example/json/version",
        "ws://127.0.0.1:9222/devtools/browser/opaque-id",
        "wss://browser.example/devtools/browser/1234",
    ] {
        validate_cdp_url(value).unwrap();
    }
    for value in [
        "http://user:secret@example.com",
        "ws://example.com/devtools/page/1234",
        "ws://example.com/devtools/browser/",
        "ws://example.com/devtools/browser/a/b",
        "http://example.com/?token=secret",
        "http://example.com/#secret",
        "http://example.com/browser/launch",
        "file:///tmp/browser",
        "not-a-url",
        "http://example.com/\n",
    ] {
        let error = validate_cdp_url(value).unwrap_err();
        assert!(!error.to_string().contains("secret"));
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    }
    assert!(validate_cdp_url(&"x".repeat(4_097)).is_err());
    let mut candidate = config();
    candidate.backend = BrowserBackendConfig::Cdp {
        url: "http://127.0.0.1:9222".into(),
        authorization: Some(SecretReference {
            env: Some("BROWSER_TOKEN".into()),
            file: None,
        }),
    };
    candidate.validate().unwrap();
    candidate
        .resolve_paths(std::path::Path::new("/opt/ocd"))
        .unwrap();
    let BrowserBackendConfig::Cdp { authorization, .. } = &mut candidate.backend else {
        unreachable!()
    };
    *authorization = Some(SecretReference {
        env: None,
        file: Some("browser-token".into()),
    });
    candidate
        .resolve_paths(std::path::Path::new("/opt/ocd"))
        .unwrap();
    candidate.validate().unwrap();
    let BrowserBackendConfig::Cdp { authorization, .. } = &mut candidate.backend else {
        unreachable!()
    };
    assert_eq!(
        authorization.as_ref().unwrap().file.as_deref(),
        Some(std::path::Path::new("/opt/ocd/browser-token"))
    );
    authorization.as_mut().unwrap().env = Some("invalid token reference".into());
    assert!(candidate.validate().is_err());
}
