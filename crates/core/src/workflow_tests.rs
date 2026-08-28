use super::*;

#[test]
fn workflow_v2_capacity_defaults_preserve_legacy_serialization() {
    let legacy = r#"{"max_in_flight_requests":64,"max_steps":1024,"max_state_bytes":33554432,"max_instances_per_account":10000,"max_instances_per_definition":10000,"max_active_per_account":1000,"max_account_state_bytes":1073741824,"lease_ms":60000,"heartbeat_ms":20000,"dispatch_timeout_ms":300000,"recovery_backoff_ms":1000,"creation_grace_ms":60000}"#;
    let default = WorkflowsConfig::default();
    assert_eq!(serde_json::to_string(&default).unwrap(), legacy);
    assert_eq!(
        serde_json::from_str::<WorkflowsConfig>(legacy).unwrap(),
        default
    );
    let altered = WorkflowsConfig {
        max_parallel_steps: 1,
        max_buffered_events: 2,
        max_event_bytes: 1024,
        ..default
    };
    altered.validate().unwrap();
    let encoded = serde_json::to_string(&altered).unwrap();
    assert_eq!(
        serde_json::from_str::<WorkflowsConfig>(&encoded).unwrap(),
        altered
    );
    for field in [
        "max_parallel_steps",
        "max_buffered_events",
        "max_event_bytes",
    ] {
        let mut invalid = serde_json::to_value(&altered).unwrap();
        invalid[field] = serde_json::json!(0);
        assert!(
            serde_json::from_value::<WorkflowsConfig>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[test]
fn durable_failure_vocabulary_keeps_unknown_and_stale_outcomes_nonterminal() {
    for code in [
        ErrorCode::WorkflowStepTimeout,
        ErrorCode::WorkflowStepRetriesExhausted,
        ErrorCode::WorkflowNonRetryable,
        ErrorCode::WorkflowEventTimeout,
        ErrorCode::WorkflowDurationInvalid,
        ErrorCode::WorkflowEventTypeInvalid,
    ] {
        assert_eq!(terminal_error_code_v2(code.as_str()).unwrap(), code);
        assert!(terminal_error_code(code.as_str()).is_err());
    }
    for code in [
        ErrorCode::WorkflowRunStale,
        ErrorCode::WorkflowStepStale,
        ErrorCode::WorkflowRuntimeUnavailable,
        ErrorCode::WorkflowInstanceBusy,
        ErrorCode::WorkflowEventQueueFull,
    ] {
        assert!(terminal_error_code_v2(code.as_str()).is_err());
    }
    assert!(terminal_error_code_v2("private exception text").is_err());
}

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

#[test]
fn workflow_duration_shared_javascript_fixtures() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../runtime/tests/fixtures/workflow-duration.json"
    ))
    .unwrap();
    for fixture in fixtures.as_array().unwrap() {
        let result = duration_ms(
            &fixture["input"],
            fixture["maximum"]
                .as_u64()
                .unwrap_or(WORKFLOW_MAX_DURATION_MS),
        );
        match fixture["expected"].as_u64() {
            Some(expected) => assert_eq!(result.unwrap(), expected, "{fixture}"),
            None => assert_eq!(
                result.unwrap_err().code(),
                ErrorCode::WorkflowDurationInvalid,
                "{fixture}"
            ),
        }
    }
    assert_eq!(
        duration_ms(
            &serde_json::json!(format!("0.{}1 weeks", "0".repeat(4000))),
            WORKFLOW_MAX_DURATION_MS
        )
        .unwrap(),
        1
    );
    assert!(
        duration_ms(
            &serde_json::json!(format!("{} ms", "0".repeat(4096))),
            WORKFLOW_MAX_DURATION_MS
        )
        .is_err()
    );
    for value in [
        0,
        -1,
        WORKFLOW_MAX_SAFE_INTEGER as i64,
        -(WORKFLOW_MAX_SAFE_INTEGER as i64),
    ] {
        assert_eq!(timestamp_ms(&serde_json::json!(value)).unwrap(), value);
    }
    for invalid in [
        serde_json::json!(0.5),
        serde_json::json!(WORKFLOW_MAX_SAFE_INTEGER + 1),
        serde_json::json!("1970-01-01"),
        serde_json::Value::Null,
    ] {
        assert_eq!(
            timestamp_ms(&invalid).unwrap_err().code(),
            ErrorCode::WorkflowDurationInvalid
        );
    }
}

#[test]
fn workflow_v2_config_is_resolved_strict_frozen_and_bounded() {
    use serde_json::json;
    let config = WorkflowStepConfig::resolve(&json!({})).unwrap();
    assert_eq!(config, WorkflowStepConfig::default());
    assert_eq!(
        config.canonical_json().unwrap(),
        r#"{"retries":{"backoff":"exponential","delay":10000,"limit":5},"timeout":60000}"#
    );
    assert!(!config.fits_activation(89_999));
    assert!(config.fits_activation(90_000));
    let resolved=WorkflowStepConfig::resolve(&json!({"timeout":"1 minute","retries":{"limit":1.0,"delay":"0.0001 second","backoff":"linear"}})).unwrap();
    assert_eq!(resolved.timeout, 60_000);
    assert_eq!(resolved.retries.limit, 1);
    assert_eq!(resolved.retries.delay, 1);
    assert_eq!(resolved.retries.backoff, WorkflowBackoff::Linear);
    for invalid in [
        json!(null),
        json!([]),
        json!({"unknown":true}),
        json!({"rollback":false}),
        json!({"timeout":0}),
        json!({"timeout":240001}),
        json!({"timeout":"five minutes"}),
        json!({"retries":null}),
        json!({"retries":{}}),
        json!({"retries":{"limit":1}}),
        json!({"retries":{"delay":1}}),
        json!({"retries":{"limit":101,"delay":0}}),
        json!({"retries":{"limit":-1,"delay":0}}),
        json!({"retries":{"limit":0.5,"delay":0}}),
        json!({"retries":{"limit":1,"delay":0,"backoff":"dynamic"}}),
        json!({"retries":{"limit":1,"delay":0,"unknown":false}}),
    ] {
        assert!(WorkflowStepConfig::resolve(&invalid).is_err(), "{invalid}");
    }
    // Persisted resolved configs require all fields; serde must not silently
    // apply new defaults to an old descriptor or hide corrupt stored values.
    assert!(serde_json::from_str::<WorkflowStepConfig>(r#"{"timeout":1}"#).is_err());
    let corrupt: WorkflowStepConfig = serde_json::from_value(
        json!({"timeout":0,"retries":{"limit":5,"delay":1,"backoff":"constant"}}),
    )
    .unwrap();
    assert!(corrupt.validate().is_err());
    assert!(corrupt.canonical_json().is_err());
    assert!(
        WorkflowStepConfig::resolve(&json!({"timeout":1,"retries":{"limit":0,"delay":0}})).is_ok()
    );
}

#[test]
fn workflow_retry_formulas_saturate_without_float_or_attempt_overflow() {
    for (backoff, expected) in [
        (WorkflowBackoff::Constant, [10, 10, 10, 10]),
        (WorkflowBackoff::Linear, [10, 20, 30, 40]),
        (WorkflowBackoff::Exponential, [10, 20, 40, 80]),
    ] {
        let policy = WorkflowRetryPolicy {
            limit: 100,
            delay: 10,
            backoff,
        };
        for (index, expected) in expected.into_iter().enumerate() {
            assert_eq!(policy.delay_after(index as u32 + 1).unwrap(), expected);
        }
        assert!(policy.delay_after(0).is_err());
        assert!(policy.delay_after(102).is_err());
    }
    let mut policy = WorkflowRetryPolicy {
        limit: 100,
        delay: WORKFLOW_MAX_DURATION_MS,
        backoff: WorkflowBackoff::Exponential,
    };
    for attempt in [1, 2, 31, 64, 65, 100, 101] {
        assert_eq!(
            policy.delay_after(attempt).unwrap(),
            WORKFLOW_MAX_RETRY_DELAY_MS
        );
    }
    policy.delay = 0;
    assert_eq!(policy.delay_after(101).unwrap(), 0);
    policy.delay = WORKFLOW_MAX_DURATION_MS + 1;
    assert!(policy.delay_after(1).is_err());
    policy.delay = 1;
    policy.limit = 101;
    assert!(policy.delay_after(1).is_err());
}

#[test]
fn workflow_retention_freezes_defaults_and_checked_expiry() {
    use serde_json::json;
    let defaults = WorkflowRetention::resolve(&json!({}), &WorkflowRetention::default()).unwrap();
    assert_eq!(defaults, WorkflowRetention::default());
    assert_eq!(
        defaults.expires_at(123, true).unwrap(),
        123 + 7 * 86_400_000
    );
    assert_eq!(
        defaults.expires_at(123, false).unwrap(),
        123 + 30 * 86_400_000
    );
    assert!(defaults.expires_at(i64::MAX, true).is_err());
    assert!(
        defaults
            .expires_at(WORKFLOW_MAX_SAFE_INTEGER as i64, false)
            .is_err()
    );
    let custom = WorkflowRetention::resolve(
        &json!({"successRetention":"1 hour","errorRetention":"365 days"}),
        &defaults,
    )
    .unwrap();
    assert_eq!(custom.success_retention_ms, 3_600_000);
    assert_eq!(custom.error_retention_ms, WORKFLOW_MAX_DURATION_MS);
    for invalid in [
        json!(null),
        json!({"ttl":"1 hour"}),
        json!({"successRetention":3599999}),
        json!({"errorRetention":"366 days"}),
    ] {
        assert!(
            WorkflowRetention::resolve(&invalid, &defaults).is_err(),
            "{invalid}"
        );
    }
    let override_one =
        WorkflowRetention::resolve(&json!({"successRetention":"2 hours"}), &custom).unwrap();
    assert_eq!(override_one.error_retention_ms, custom.error_retention_ms);
    assert_eq!(override_one.success_retention_ms, 7_200_000);
    let mut config = WorkflowsConfig::default();
    assert!(
        serde_json::to_value(&config)
            .unwrap()
            .get("default_retention")
            .is_none()
    );
    config.default_retention = custom;
    config.validate().unwrap();
    let encoded = serde_json::to_value(&config).unwrap();
    assert!(encoded.get("default_retention").is_some());
    assert_eq!(
        serde_json::from_value::<WorkflowsConfig>(encoded).unwrap(),
        config
    );
    config.default_retention.success_retention_ms = 1;
    assert!(config.validate().is_err());
}
