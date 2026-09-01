use super::*;

fn declaration(kind: WorkflowStepKind, config: Value) -> WorkflowStepDeclaration {
    WorkflowStepDeclaration {
        ordinal: 0,
        kind,
        name: "step".into(),
        name_count: 1,
        config,
        rollback_config: None,
        rollback_step: false,
        dependencies: vec![],
        batch_first_ordinal: 0,
        batch_size: 1,
    }
}

#[test]
fn public_defaults_and_duration_aliases_resolve_once_and_stored_policy_is_never_repaired() {
    for (kind, raw, expected) in [
        (
            WorkflowStepKind::Do,
            json!({}),
            json!({"retries":{"limit":5,"delay":10000,"backoff":"exponential"},"timeout":60000}),
        ),
        (
            WorkflowStepKind::Sleep,
            json!({"duration":"0.001 seconds"}),
            json!({"durationMs":1}),
        ),
        (
            WorkflowStepKind::SleepUntil,
            json!({"timestamp":-1}),
            json!({"timestampMs":-1}),
        ),
        (
            WorkflowStepKind::WaitEvent,
            json!({"type":"approved"}),
            json!({"type":"approved","timeoutMs":86400000}),
        ),
        (
            WorkflowStepKind::WaitEvent,
            json!({"type":"approved","timeout":0}),
            json!({"type":"approved","timeoutMs":0}),
        ),
    ] {
        let descriptor = declaration(kind, raw).resolve().unwrap();
        assert_eq!(
            descriptor.config.canonical_json().unwrap(),
            expected.to_string()
        );
        assert_eq!(
            WorkflowDurableConfig::from_canonical(kind, &expected.to_string()).unwrap(),
            descriptor.config
        );
        assert_eq!(
            WorkflowDurableConfig::from_canonical(kind, "{}")
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInvariantViolation
        );
        assert!(WorkflowDurableConfig::from_canonical(kind, &format!(" {expected}")).is_err());
    }
    for (kind, value) in [
        (WorkflowStepKind::Do, json!({"rollback":false})),
        (
            WorkflowStepKind::Sleep,
            json!({"duration":1,"unexpected":true}),
        ),
        (WorkflowStepKind::SleepUntil, json!({"timestamp":0.5})),
        (WorkflowStepKind::WaitEvent, json!({"type":"a.b"})),
        (WorkflowStepKind::WaitEvent, json!({"type":"-bad"})),
        (WorkflowStepKind::WaitEvent, json!({"type":"好"})),
        (WorkflowStepKind::WaitEvent, json!({"type":"a".repeat(101)})),
        (
            WorkflowStepKind::WaitEvent,
            json!({"type":"ok","timeout":null}),
        ),
    ] {
        assert!(declaration(kind, value).resolve().is_err());
    }
    for value in [
        json!({"timeout":60000}),
        json!({"retries":{"limit":5,"delay":10000},"timeout":60000}),
        json!({"retries":{"limit":5,"delay":10000,"backoff":"exponential"},"timeout":0}),
    ] {
        assert!(
            WorkflowDurableConfig::from_canonical(WorkflowStepKind::Do, &value.to_string())
                .is_err()
        );
    }
    assert!(validate_workflow_event_type("_A-9").is_ok());
}

#[test]
fn replay_digest_contains_kind_policy_batch_and_ordered_frontier() {
    let mut request = declaration(WorkflowStepKind::Do, json!({"timeout":"1 second"}));
    request.ordinal = 3;
    request.batch_first_ordinal = 3;
    request.batch_size = 2;
    request.dependencies = vec![1, 2];
    let original = request.clone().resolve().unwrap();
    let digest = original.sha256().unwrap();
    request.config = json!({"timeout":1000});
    assert_eq!(request.resolve().unwrap().sha256().unwrap(), digest);
    let variants = [
        WorkflowStepDescriptor {
            name: "other".into(),
            ..original.clone()
        },
        WorkflowStepDescriptor {
            name_count: 2,
            ..original.clone()
        },
        WorkflowStepDescriptor {
            batch_size: 1,
            ..original.clone()
        },
        WorkflowStepDescriptor {
            dependencies: vec![2],
            ..original.clone()
        },
        WorkflowStepDescriptor {
            config: WorkflowDurableConfig::Do(WorkflowStepConfig::default()),
            ..original.clone()
        },
        WorkflowStepDescriptor {
            rollback_config: Some(WorkflowStepConfig::default()),
            ..original.clone()
        },
        WorkflowStepDescriptor {
            ordinal: 4,
            ..original.clone()
        },
    ];
    for changed in variants {
        assert_ne!(changed.sha256().unwrap(), digest);
    }
    for predecessors in [vec![1, 1, 2], vec![2, 1], vec![1, 2, 3]] {
        assert!(
            WorkflowStepDescriptor {
                dependencies: predecessors,
                ..original.clone()
            }
            .sha256()
            .is_err()
        );
    }
    for predecessors in [vec![], vec![0, 2]] {
        assert!(
            WorkflowStepDescriptor {
                dependencies: predecessors,
                ..original.clone()
            }
            .sha256()
            .is_ok()
        );
    }
    assert_ne!(
        WorkflowStepDescriptor {
            config: WorkflowDurableConfig::Sleep(0),
            ..original.clone()
        }
        .sha256()
        .unwrap(),
        digest
    );
    let mut rollback = declaration(WorkflowStepKind::Do, json!({}));
    rollback.rollback_config = Some(json!({"timeout":"1 minute"}));
    assert_eq!(
        rollback.resolve().unwrap().rollback_config.unwrap().timeout,
        60_000
    );
    let mut invalid_rollback = declaration(WorkflowStepKind::Sleep, json!({"duration":0}));
    invalid_rollback.rollback_config = Some(json!({}));
    assert!(invalid_rollback.resolve().is_err());
    assert!(
        WorkflowStepDescriptor {
            batch_size: 17,
            ..original.clone()
        }
        .sha256()
        .is_err()
    );
    assert!(
        WorkflowStepDescriptor {
            ordinal: 1024,
            ..original.clone()
        }
        .sha256()
        .is_err()
    );
    assert!(
        WorkflowStepDescriptor {
            name: "好".repeat(86),
            ..original
        }
        .sha256()
        .is_err()
    );
}

#[test]
fn logical_accounting_uses_the_shared_current_contract_and_counts_utf8_and_edges() {
    let contract: Value =
        serde_json::from_str(include_str!("../../../../share/workflow-accounting.json")).unwrap();
    for (field, actual) in [
        ("instanceFixedBytes", WORKFLOW_INSTANCE_BYTES),
        ("stepFixedBytes", WORKFLOW_STEP_BYTES),
        ("dependencyBytes", WORKFLOW_DEPENDENCY_BYTES),
        ("eventFixedBytes", WORKFLOW_EVENT_BYTES),
    ] {
        assert_eq!(contract[field], json!(actual));
    }
    let mut descriptor = declaration(WorkflowStepKind::Sleep, json!({"duration":0}))
        .resolve()
        .unwrap();
    descriptor.name = "等待".into();
    descriptor.ordinal = 2;
    descriptor.batch_first_ordinal = 2;
    descriptor.dependencies = vec![0, 1];
    assert_eq!(
        descriptor.state_bytes().unwrap(),
        160 + 6 + r#"{"durationMs":0,"rollbackStep":false}"#.len() + 2 * 16
    );
}

#[test]
fn restart_selector_and_stored_policy_validation_are_strict() {
    assert_eq!(WorkflowRestartStepType::Do.as_str(), "do");
    assert_eq!(WorkflowRestartStepType::Sleep.as_str(), "sleep");
    assert_eq!(
        WorkflowRestartStepType::WaitForEvent.as_str(),
        "waitForEvent"
    );
    assert!(WorkflowRestartStepType::Do.matches(WorkflowStepKind::Do));
    assert!(WorkflowRestartStepType::Sleep.matches(WorkflowStepKind::Sleep));
    assert!(WorkflowRestartStepType::Sleep.matches(WorkflowStepKind::SleepUntil));
    assert!(WorkflowRestartStepType::WaitForEvent.matches(WorkflowStepKind::WaitEvent));
    assert!(!WorkflowRestartStepType::Do.matches(WorkflowStepKind::Sleep));

    let defaulted: WorkflowRestartSelector =
        serde_json::from_value(json!({"name":"step"})).unwrap();
    assert_eq!(defaulted.count, 1);
    defaulted.validate().unwrap();
    for selector in [
        WorkflowRestartSelector {
            name: String::new(),
            count: 1,
            step_type: None,
        },
        WorkflowRestartSelector {
            name: "x".repeat(257),
            count: 1,
            step_type: None,
        },
        WorkflowRestartSelector {
            name: "step".into(),
            count: 0,
            step_type: None,
        },
        WorkflowRestartSelector {
            name: "step".into(),
            count: 1025,
            step_type: None,
        },
    ] {
        assert_eq!(
            selector.validate().unwrap_err().code(),
            ErrorCode::WorkflowMethodUnsupported
        );
    }
    assert_eq!(
        WorkflowDurableConfig::from_canonical(WorkflowStepKind::Sleep, &"x".repeat(4097))
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        WorkflowDurableConfig::Sleep(WORKFLOW_MAX_DURATION_MS + 1)
            .validate()
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowDurationInvalid
    );
    assert!(WorkflowDurableConfig::resolve(WorkflowStepKind::Sleep, &json!({})).is_err());

    let resolved = declaration(WorkflowStepKind::Do, json!({}))
        .resolve()
        .unwrap();
    assert_eq!(
        WorkflowStepDescriptor {
            rollback_config: Some(WorkflowStepConfig::default()),
            rollback_step: true,
            ..resolved.clone()
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowStepConfigUnsupported
    );
    assert_eq!(
        WorkflowStepDescriptor {
            config: WorkflowDurableConfig::Sleep(0),
            rollback_config: Some(WorkflowStepConfig::default()),
            ..resolved.clone()
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowStepConfigUnsupported
    );
    assert_eq!(
        WorkflowStepDescriptor {
            config: WorkflowDurableConfig::Sleep(0),
            rollback_step: true,
            ..resolved
        }
        .validate()
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowStepConfigUnsupported
    );
}
