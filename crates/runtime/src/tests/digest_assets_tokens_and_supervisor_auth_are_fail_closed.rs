use super::*;

#[test]
fn digest_assets_tokens_and_supervisor_auth_are_fail_closed() {
    assert_eq!(
        load_assets(Path::new("relative-assets"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    assert_eq!(
        load_assets(dir.path()).unwrap_err().code(),
        ErrorCode::ConfigCompileFailed
    );
    fs::write(dir.path().join("dist/worker.js"), b"export default {}").unwrap();
    fs::create_dir_all(dir.path().join("dist/ai")).unwrap();
    fs::create_dir_all(dir.path().join("dist/ai-search")).unwrap();
    fs::write(dir.path().join("dist/ai/index.js"), b"export default {}").unwrap();
    fs::write(
        dir.path().join("dist/ai-search/index.js"),
        b"export default {}",
    )
    .unwrap();
    let (_, workers, config_path) = load_assets(dir.path()).unwrap();
    assert_eq!(
        workers
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "dist/ai-search/index.js",
            "dist/ai/index.js",
            "dist/worker.js"
        ]
    );
    assert_eq!(config_path, dir.path().join("config.capnp"));

    let valid = SecretString::new(TOKEN);
    let binding = SecretString::new(TOKEN_B);
    let rendered = render_config_with_tokens(
        &format!(
            "{}:{}:{}",
            RuntimeLock::token_placeholder(),
            BINDING_TOKEN_PLACEHOLDER,
            OBSERVABILITY_TOKEN_PLACEHOLDER
        ),
        &valid,
        &binding,
        &SecretString::new(TOKEN_C),
    )
    .unwrap();
    assert_eq!(rendered, format!("{TOKEN}:{TOKEN_B}:{TOKEN_C}"));
    assert_eq!(
        render_config_with_tokens(
            &format!(
                "{}:{}:{}",
                RuntimeLock::token_placeholder(),
                BINDING_TOKEN_PLACEHOLDER,
                OBSERVABILITY_TOKEN_PLACEHOLDER
            ),
            &valid,
            &valid,
            &SecretString::new(TOKEN_C),
        )
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );
    for token in [
        SecretString::new("short"),
        SecretString::new("g".repeat(64)),
        SecretString::new("A".repeat(64)),
    ] {
        assert_eq!(
            validate_token(&token).unwrap_err().code(),
            ErrorCode::RuntimeInvalid
        );
    }

    use crate::supervisor::{
        ExternalServiceAddress, GenerationAuthRegistry, SupervisorSnapshot, SupervisorState,
        generate_internal_token, token_fingerprint,
    };
    assert!(
        ExternalServiceAddress::loopback("runtime-source", "127.0.0.1:8080".parse().unwrap())
            .is_ok()
    );
    for (name, address) in [
        ("", "127.0.0.1:8080"),
        (&"x".repeat(65), "127.0.0.1:8080"),
        ("bad name", "127.0.0.1:8080"),
        ("valid", "127.0.0.1:0"),
        ("valid", "192.0.2.1:8080"),
    ] {
        assert!(ExternalServiceAddress::loopback(name, address.parse().unwrap()).is_err());
    }
    let states = [
        (SupervisorState::Stopped, "STOPPED"),
        (SupervisorState::Starting, "STARTING"),
        (SupervisorState::Running, "RUNNING"),
        (SupervisorState::BackingOff, "BACKING_OFF"),
        (SupervisorState::Failed, "FAILED"),
        (SupervisorState::Draining, "DRAINING"),
        (SupervisorState::Stopping, "STOPPING"),
    ];
    for (state, expected) in states {
        assert_eq!(state.as_str(), expected);
    }
    let snapshot = SupervisorSnapshot::initial(SystemTime::UNIX_EPOCH, "digest".into());
    assert_eq!(snapshot.state, SupervisorState::Stopped);
    assert_eq!(snapshot.reason, ReadinessReason::Starting);
    assert!(!format!("{snapshot:?}").contains("listen_port"));

    let auth = GenerationAuthRegistry::new();
    assert!(!auth.authorize(TOKEN, "generation"));
    assert!(auth.credential().is_none());
    assert!(auth.active_fingerprint().is_none());
    auth.activate(valid.clone());
    let credential = auth.credential().unwrap();
    assert_eq!(credential.expose(), TOKEN);
    assert_eq!(
        format!("{credential:?}"),
        "GenerationCredential([REDACTED])"
    );
    assert!(!format!("{auth:?}").contains(TOKEN));
    for generation in ["", "\n", &"x".repeat(129)] {
        assert!(!auth.authorize(TOKEN, generation));
    }
    assert!(!auth.authorize("bad", "generation"));
    assert!(!auth.authorize(TOKEN_B, "generation"));
    assert!(auth.authorize(TOKEN, "generation"));
    assert!(auth.authorize(TOKEN, "generation"));
    assert!(!auth.authorize(TOKEN, "other"));
    assert_eq!(auth.active_fingerprint(), Some(token_fingerprint(&valid)));
    auth.clear();
    assert!(auth.credential().is_none());

    let generated = generate_internal_token().unwrap();
    validate_token(&generated).unwrap();
    assert_eq!(token_fingerprint(&generated).len(), 16);
}
