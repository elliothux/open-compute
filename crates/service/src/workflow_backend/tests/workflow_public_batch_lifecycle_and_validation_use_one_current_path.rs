use super::*;

#[test]
fn workflow_public_batch_lifecycle_and_validation_use_one_current_path() {
    let f = fixture();
    let (_definition, binding) = ready(&f);
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig::default(),
    )
    .unwrap()
    .with_metrics(f.metrics.clone());
    let path = |operation: &str| {
        format!(
            "/internal/bindings/v1/workflow/{}/{operation}",
            binding.descriptor.binding_id
        )
    };
    let created = service
        .execute(
            &path("create-batch"),
            &mutation_caller(&binding),
            json!({"instances":[
                {"id":"batch-a","payloadBase64":"T0NEVgECAA==","locationHint":"wnam",
                 "retention":{"successRetention":"1 hour","errorRetention":"2 hours"}},
                {"id":"batch-b","payloadBase64":"T0NEVgECAA==","locationHint":"apac-ne"}
            ]}),
            10,
        )
        .unwrap();
    let rows = created["instances"].as_array().unwrap();
    let first: WorkflowInstanceId = rows[0]["instanceId"].as_str().unwrap().parse().unwrap();
    let second: WorkflowInstanceId = rows[1]["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        service
            .execute(
                &path("pause"),
                &mutation_caller(&binding),
                json!({"instanceId":first}),
                11,
            )
            .unwrap(),
        json!({"ok":true})
    );
    assert_eq!(
        service
            .execute(
                &path("status"),
                &caller(&binding),
                json!({"instanceId":first}),
                11,
            )
            .unwrap()["status"],
        "paused"
    );
    for operation in ["resume", "send-event", "terminate"] {
        let body = if operation == "send-event" {
            json!({"instanceId":first,"type":"ready","payloadBase64":"T0NEVgECAw=="})
        } else {
            json!({"instanceId":first})
        };
        assert_eq!(
            service
                .execute(&path(operation), &mutation_caller(&binding), body, 12,)
                .unwrap(),
            json!({"ok":true}),
            "{operation}"
        );
    }
    assert_eq!(
        service
            .execute(
                &path("delete"),
                &mutation_caller(&binding),
                json!({"instanceId":second}),
                13,
            )
            .unwrap(),
        json!({"ok":true})
    );
    let deleted = service
        .execute(
            &path("delete-batch"),
            &mutation_caller(&binding),
            json!({"instanceIds":["batch-a","missing","batch-a"]}),
            14,
        )
        .unwrap();
    assert_eq!(deleted["deleted"].as_array().unwrap().len(), 2);
    assert_eq!(deleted["errors"][0]["id"], "missing");

    for location in [
        "wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me",
    ] {
        validate_location(Some(location)).unwrap();
    }
    assert_eq!(
        validate_location(Some("moon")).unwrap_err().code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    for code in [
        ErrorCode::WorkflowRuntimeUnavailable,
        ErrorCode::WorkflowInvariantViolation,
        ErrorCode::WorkflowInstanceAlreadyExists,
        ErrorCode::WorkflowInstanceStateConflict,
        ErrorCode::WorkflowInstanceBusy,
        ErrorCode::WorkflowInstanceCleanupPending,
        ErrorCode::WorkflowInstanceNotFound,
        ErrorCode::WorkflowStateQuotaExceeded,
        ErrorCode::WorkflowPayloadTooLarge,
        ErrorCode::WorkflowResultTooLarge,
        ErrorCode::WorkflowMethodUnsupported,
        ErrorCode::WorkflowSerializationUnsupported,
    ] {
        assert_eq!(workflow_error_code(code.as_str()).unwrap(), code);
    }
    assert!(workflow_error_code("PRIVATE_ERROR").is_err());
    let batch = WorkflowOperationId::generate();
    assert_eq!(
        workflow_named_item_operation_id(batch, "batch-a").unwrap(),
        workflow_named_item_operation_id(batch, "batch-a").unwrap()
    );
    for (operation, body) in [
        ("create-batch", json!({"instances":[]})),
        ("delete-batch", json!({"instanceIds":[]})),
        ("pause", json!({"instanceId":first,"rollback":true})),
        ("unknown", json!({})),
    ] {
        assert_eq!(
            service
                .execute(&path(operation), &mutation_caller(&binding), body, 15,)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowMethodUnsupported,
            "{operation}"
        );
    }
}
