use super::*;

#[test]
fn workflow_identity_and_private_fence_validation() {
    for id in ["a", "_a-b_1", &"a".repeat(100)] {
        validate_workflow_instance_id(id).unwrap();
    }
    for id in ["", "-a", "a/b", " a", "中文", &"a".repeat(101)] {
        assert_eq!(
            validate_workflow_instance_id(id).unwrap_err().code(),
            ErrorCode::WorkflowInstanceIdInvalid
        );
    }
    validate_workflow_name(&"a".repeat(64)).unwrap();
    assert!(validate_workflow_name(&"a".repeat(65)).is_err());
    let token = WorkflowToken::from_bytes([0xab; 32]);
    let json = serde_json::to_string(&token).unwrap();
    assert_eq!(serde_json::from_str::<WorkflowToken>(&json).unwrap(), token);
    assert_eq!(token.as_bytes(), &[0xab; 32]);
    assert!(!format!("{token:?}").contains("ab"));
    for invalid in [
        "\"ab\"",
        &format!("\"{}\"", "AB".repeat(32)),
        &format!("\"{}\"", "g".repeat(64)),
    ] {
        assert!(serde_json::from_str::<WorkflowToken>(invalid).is_err());
    }
}

#[test]
fn workflow_limits_and_heartbeat_boundaries() {
    WorkflowsConfig::default().validate().unwrap();
    let mut config = WorkflowsConfig {
        heartbeat_ms: 30000,
        ..WorkflowsConfig::default()
    };
    assert!(config.validate().is_err());
    config.heartbeat_ms = 29999;
    config.validate().unwrap();
    config.dispatch_timeout_ms = 30000;
    assert!(config.validate().is_err());
    config.dispatch_timeout_ms = 30001;
    config.validate().unwrap();
    config.max_steps = 1025;
    assert!(config.validate().is_err());
}
