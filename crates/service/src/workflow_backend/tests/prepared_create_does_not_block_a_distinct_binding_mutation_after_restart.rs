use super::*;

#[test]
fn prepared_create_does_not_block_a_distinct_binding_mutation_after_restart() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let interrupted_operation = WorkflowOperationId::generate();
    let interrupted_request = json!({"id":"interrupted","payloadBase64":"T0NEVgECAA=="});
    let interrupted_json = serde_json::to_vec(&interrupted_request).unwrap();
    let repository = WorkflowRepository::new(f.storage.db());
    let fingerprint = workflow_binding_operation_fingerprint(
        binding.descriptor.binding_id,
        "create",
        &interrupted_json,
    );
    assert!(
        repository
            .begin_binding_operation(
                binding.descriptor.binding_id,
                interrupted_operation,
                "create",
                &fingerprint,
                &interrupted_json,
                10,
            )
            .unwrap()
            .is_none()
    );
    let interrupted = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            interrupted_operation,
            Some("interrupted"),
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
    let next = restarted
        .execute(
            &path,
            &mutation_caller(&binding),
            json!({"id":"after-restart","payloadBase64":"T0NEVgECAA=="}),
            12,
        )
        .unwrap();
    assert_eq!(next["id"], "after-restart");

    let replay = restarted
        .execute(
            &path,
            &mutation_caller_for(&binding, interrupted_operation),
            interrupted_request,
            13,
        )
        .unwrap();
    assert_eq!(
        replay["instanceId"].as_str().unwrap(),
        interrupted.instance_id.to_string()
    );
    assert_eq!(
        repository
            .find_instance(definition, "after-restart")
            .unwrap()
            .identity
            .external_instance_id,
        "after-restart"
    );
}
