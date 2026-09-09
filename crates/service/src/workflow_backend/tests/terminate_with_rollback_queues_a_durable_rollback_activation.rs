use super::*;

#[test]
fn terminate_with_rollback_queues_a_durable_rollback_activation() {
    let f = fixture();
    let (_definition, binding) = ready(&f);
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig::default(),
    )
    .unwrap();
    let path = |operation: &str| {
        format!(
            "/internal/bindings/v1/workflow/{}/{operation}",
            binding.descriptor.binding_id
        )
    };
    let created = service
        .execute(
            &path("create"),
            &mutation_caller(&binding),
            json!({"id":"rollback-instance","payloadBase64":"T0NEVgECAA=="}),
            10,
        )
        .unwrap();
    let instance_id: WorkflowInstanceId = created["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        service
            .execute(
                &path("terminate"),
                &mutation_caller(&binding),
                json!({"instanceId":instance_id,"rollback":true}),
                11,
            )
            .unwrap(),
        json!({"ok":true})
    );
    let record = f.scheduler.workflow_instance(instance_id).unwrap().unwrap();
    assert_eq!(record.state, WorkflowState::Queued);
    assert!(record.durable.rollback_requested);
    assert!(
        f.scheduler
            .claim_workflow(&record.identity, 11, &WorkflowsConfig::default())
            .unwrap()
            .unwrap()
            .rollback
    );
}
