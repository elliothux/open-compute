use super::*;
use crate::workflow_http::tests::{Fixture, fixture};
use open_compute_core::{RequestId, SecretString, WorkflowId, WorkflowOperationId};
use open_compute_storage::scheduler::{WorkflowCompletion, WorkflowState};
use open_compute_storage::{
    NewVersion, NewVersionProducts, WorkerRepository, WorkflowBindingRecord,
};
use serde_json::json;

fn ready(f: &Fixture) -> (WorkflowId, WorkflowBindingRecord) {
    let repository = WorkflowRepository::new(f.storage.db());
    let definition = repository
        .create_definition(f.account, "backend", 0)
        .unwrap();
    let version = repository
        .stage_version(f.account, definition.id, f.version, "Flow", 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.workflow_version_id, true, 2)
        .unwrap();
    (definition.id, ready_binding(f, definition.id))
}

fn ready_binding(f: &Fixture, definition: WorkflowId) -> WorkflowBindingRecord {
    let repository = WorkflowRepository::new(f.storage.db());
    let workers = WorkerRepository::new(f.storage.db());
    let (worker, _) = workers
        .create_worker(
            f.account,
            &format!("caller-{}", RequestId::generate()),
            RequestId::generate(),
            0,
            1_000_000,
        )
        .unwrap();
    let version = VersionId::generate();
    let binding = repository
        .prepare_binding(
            f.account,
            version,
            "FLOW",
            definition,
            "Flow",
            None,
            None,
            Vec::new(),
            3,
        )
        .unwrap();
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: f.account,
                worker_id: worker.id,
                content_kind: open_compute_storage::VersionContentKind::Worker,
                artifact_sha256: Some([3; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [4; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 3,
            },
            &NewVersionProducts {
                workflow_bindings: std::slice::from_ref(&binding),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 4).unwrap();
    binding
}

fn caller(binding: &WorkflowBindingRecord) -> HeaderMap {
    HeaderMap::from_iter([
        (
            HeaderName::from_static("x-open-compute-version-id"),
            HeaderValue::from_str(&binding.version_id.to_string()).unwrap(),
        ),
        (
            HeaderName::from_static("x-open-compute-descriptor-sha256"),
            HeaderValue::from_str(&hex::encode(binding.descriptor_sha256)).unwrap(),
        ),
        (
            HeaderName::from_static("x-open-compute-workflow-do-context"),
            HeaderValue::from_static("0"),
        ),
    ])
}

fn mutation_caller(binding: &WorkflowBindingRecord) -> HeaderMap {
    mutation_caller_for(binding, WorkflowOperationId::generate())
}

fn mutation_caller_for(
    binding: &WorkflowBindingRecord,
    operation: WorkflowOperationId,
) -> HeaderMap {
    let mut headers = caller(binding);
    headers.insert(
        HeaderName::from_static("x-open-compute-request-id"),
        HeaderValue::from_str(&operation.to_string()).unwrap(),
    );
    headers
}

fn body(fence: &WorkflowFence, fields: Value) -> Value {
    let Value::Object(mut body) = fields else {
        panic!("test body must be an object");
    };
    body.extend(
        serde_json::to_value(fence)
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
    );
    Value::Object(body)
}

mod workflow_batch_item_operation_ids_are_stable_and_instance_scoped;

mod prepared_create_replay_reuses_the_committed_per_instance_operation;

mod prepared_create_does_not_block_a_distinct_binding_mutation_after_restart;

mod prepared_create_batch_replay_reuses_the_atomic_committed_group;

mod terminate_with_rollback_queues_a_durable_rollback_activation;

mod workflow_public_batch_lifecycle_and_validation_use_one_current_path;

mod workflow_caller_uses_current_definition_scope_and_strict_handles;

mod workflow_backend_binding_scope_do_fence_and_private_step_protocol;

mod workflow_private_dynamic_delay_round_trip_is_durable;

mod workflow_private_protocol_rejects_shape_drift_and_persists_failure_delay;

mod workflow_private_http_is_bounded_and_rechecks_startup_generation;

mod workflow_metric_guards_count_all_outcomes_without_sensitive_labels;
