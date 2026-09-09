use super::*;

#[test]
fn prepared_create_replay_reuses_the_committed_per_instance_operation() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let operation = WorkflowOperationId::generate();
    let request = json!({"id":"prepared-one","payloadBase64":"T0NEVgECAA=="});
    let request_json = serde_json::to_vec(&request).unwrap();
    let repository = WorkflowRepository::new(f.storage.db());
    let fingerprint = workflow_binding_operation_fingerprint(
        binding.descriptor.binding_id,
        "create",
        &request_json,
    );
    assert!(
        repository
            .begin_binding_operation(
                binding.descriptor.binding_id,
                operation,
                "create",
                &fingerprint,
                &request_json,
                10,
            )
            .unwrap()
            .is_none()
    );
    let committed = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            operation,
            Some("prepared-one"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            11,
        )
        .unwrap();
    let restarted =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config).unwrap();
    let path = format!(
        "/internal/bindings/v1/workflow/{}/create",
        binding.descriptor.binding_id
    );
    let headers = mutation_caller_for(&binding, operation);
    let response = restarted
        .execute(&path, &headers, request.clone(), 12)
        .unwrap();
    assert_eq!(
        response["instanceId"].as_str().unwrap(),
        committed.instance_id.to_string()
    );
    assert_eq!(
        restarted.execute(&path, &headers, request, 13).unwrap(),
        response
    );
    let stored = repository
        .find_instance(definition, "prepared-one")
        .unwrap();
    assert_eq!(stored.identity, committed);
    assert_eq!(stored.identity.creation_operation_id, operation);
}
