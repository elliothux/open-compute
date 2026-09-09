use super::*;

#[test]
fn workflow_caller_uses_current_definition_scope_and_strict_handles() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let second = ready_binding(&f, definition);
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig::default(),
    )
    .unwrap();
    let path = |binding: &WorkflowBindingRecord, operation: &str| {
        format!(
            "/internal/bindings/v1/workflow/{}/{operation}",
            binding.descriptor.binding_id
        )
    };
    let created = service
        .execute(
            &path(&binding, "create"),
            &mutation_caller(&binding),
            json!({"id":"original","payloadBase64":"T0NEVgECAA=="}),
            10,
        )
        .unwrap();
    let instance_id: WorkflowInstanceId = created["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(created["id"], "original");
    assert_eq!(
        service
            .execute(
                &path(&second, "get"),
                &caller(&second),
                json!({"id":"original"}),
                11,
            )
            .unwrap(),
        json!({"id":"original","instanceId":instance_id})
    );
    assert_eq!(
        service
            .execute(
                &path(&second, "status"),
                &caller(&second),
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap(),
        json!({"status":"queued"})
    );
    for invalid in [
        json!({"id":"original"}),
        json!({"instanceId":instance_id,"id":"original"}),
    ] {
        assert_eq!(
            service
                .execute(&path(&second, "status"), &caller(&second), invalid, 11)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowSerializationUnsupported
        );
    }
    let mut do_headers = caller(&second);
    do_headers.insert(
        "x-open-compute-workflow-do-context",
        HeaderValue::from_static("1"),
    );
    assert_eq!(
        service
            .execute(
                &path(&second, "status"),
                &do_headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap()["status"],
        "queued"
    );
    let repository = WorkflowRepository::new(f.storage.db());
    let foreign = repository
        .create_definition(f.account, "foreign", 12)
        .unwrap();
    assert_eq!(
        WorkflowController::new(&f.storage, &f.scheduler, &WorkflowsConfig::default())
            .status(f.account, foreign.id, instance_id, 12)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
}
