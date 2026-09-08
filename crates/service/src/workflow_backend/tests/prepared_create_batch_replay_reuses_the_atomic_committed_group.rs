use super::*;

#[test]
fn prepared_create_batch_replay_reuses_the_atomic_committed_group() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let batch = WorkflowOperationId::generate();
    let request = json!({"instances":[
        {"id":"prepared-batch-a","payloadBase64":"T0NEVgECAA=="},
        {"id":"prepared-batch-b","payloadBase64":"T0NEVgECAA=="}
    ]});
    let request_json = serde_json::to_vec(&request).unwrap();
    let repository = WorkflowRepository::new(f.storage.db());
    let fingerprint = workflow_binding_operation_fingerprint(
        binding.descriptor.binding_id,
        "create-batch",
        &request_json,
    );
    assert!(
        repository
            .begin_binding_operation(
                binding.descriptor.binding_id,
                batch,
                "create-batch",
                &fingerprint,
                &request_json,
                20,
            )
            .unwrap()
            .is_none()
    );
    let first_operation = workflow_batch_item_operation_id(batch, 0).unwrap();
    let second_operation = workflow_batch_item_operation_id(batch, 1).unwrap();
    let create_requests = [
        (
            first_operation,
            Some("prepared-batch-a"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
        ),
        (
            second_operation,
            Some("prepared-batch-b"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
        ),
    ];
    let committed = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create_batch(f.account, definition, batch, &create_requests, 21)
        .unwrap();
    let restarted =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config).unwrap();
    let path = format!(
        "/internal/bindings/v1/workflow/{}/create-batch",
        binding.descriptor.binding_id
    );
    let headers = mutation_caller_for(&binding, batch);
    let response = restarted
        .execute(&path, &headers, request.clone(), 22)
        .unwrap();
    assert_eq!(
        response["instances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["instanceId"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        committed
            .iter()
            .map(|identity| identity.instance_id.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        restarted.execute(&path, &headers, request, 23).unwrap(),
        response
    );
    for (ordinal, identity) in committed.iter().enumerate() {
        assert_eq!(identity.creation_batch_id, batch);
        assert_eq!(
            identity.creation_operation_id,
            workflow_batch_item_operation_id(batch, ordinal).unwrap()
        );
        assert_eq!(
            repository
                .find_instance(definition, &identity.external_instance_id)
                .unwrap()
                .identity,
            *identity
        );
    }
}
