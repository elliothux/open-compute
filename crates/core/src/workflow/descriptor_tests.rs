use super::*;

fn declaration(kind: WorkflowStepKind, config: Value) -> WorkflowStepDeclaration {
    WorkflowStepDeclaration {
        ordinal: 0,
        kind,
        name: "step".into(),
        name_count: 1,
        config,
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
            ordinal: 4,
            ..original.clone()
        },
    ];
    for changed in variants {
        assert_ne!(changed.sha256().unwrap(), digest);
    }
    for predecessors in [vec![], vec![1, 1, 2], vec![2, 1], vec![0, 2], vec![1, 2, 3]] {
        assert!(
            WorkflowStepDescriptor {
                dependencies: predecessors,
                ..original.clone()
            }
            .sha256()
            .is_err()
        );
    }
    assert!(
        WorkflowStepDescriptor {
            config: WorkflowDurableConfig::Sleep(0),
            ..original.clone()
        }
        .sha256()
        .is_err()
    );
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
fn logical_accounting_uses_the_shared_v2_contract_and_counts_utf8_and_edges() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../../share/workflow-accounting-v2.json"
    ))
    .unwrap();
    for (field, actual) in [
        ("instanceFixedBytes", WORKFLOW_V2_INSTANCE_BYTES),
        ("stepFixedBytes", WORKFLOW_V2_STEP_BYTES),
        ("dependencyBytes", WORKFLOW_V2_DEPENDENCY_BYTES),
        ("eventFixedBytes", WORKFLOW_V2_EVENT_BYTES),
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
        160 + 6 + r#"{"durationMs":0}"#.len() + 2 * 16
    );
}
