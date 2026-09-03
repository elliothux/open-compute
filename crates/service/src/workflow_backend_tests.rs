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

#[test]
fn workflow_batch_item_operation_ids_are_stable_and_instance_scoped() {
    let batch = WorkflowOperationId::generate();
    let first = workflow_batch_item_operation_id(batch, 0).unwrap();
    assert_eq!(first, workflow_batch_item_operation_id(batch, 0).unwrap());
    assert_ne!(first, workflow_batch_item_operation_id(batch, 1).unwrap());
}

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

#[test]
fn workflow_backend_binding_scope_do_fence_and_private_step_protocol() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let service =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config.clone())
            .unwrap()
            .with_metrics(f.metrics.clone());
    let headers = caller(&binding);
    let path = format!(
        "/internal/bindings/v1/workflow/{}",
        binding.descriptor.binding_id
    );
    let created = service
        .execute(
            &format!("{path}/create"),
            &mutation_caller(&binding),
            json!({"id":"one","payloadBase64":"T0NEVgECEQAAAAEAAAAGc2VjcmV0BD/wAAAAAAAA"}),
            10,
        )
        .unwrap();
    let instance_id: WorkflowInstanceId = created["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(created["id"], "one");
    assert_eq!(
        service
            .execute(&format!("{path}/get"), &headers, json!({"id":"one"}), 11)
            .unwrap(),
        json!({"id":"one","instanceId":instance_id})
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/status"),
                &mutation_caller(&binding),
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap(),
        json!({"status":"queued"})
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &mutation_caller(&binding),
                json!({"id":"one","payloadBase64":"T0NEVgECAA=="}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceAlreadyExists
    );
    let mut do_headers = headers.clone();
    do_headers.insert(
        "x-open-compute-workflow-do-context",
        HeaderValue::from_static("1"),
    );
    let created_in_do = service
        .execute(
            &format!("{path}/create"),
            &{
                let mut headers = mutation_caller(&binding);
                headers.insert(
                    "x-open-compute-workflow-do-context",
                    HeaderValue::from_static("1"),
                );
                headers
            },
            json!({"id":"do","payloadBase64":"T0NEVgECAA=="}),
            11,
        )
        .unwrap();
    assert_eq!(created_in_do["id"], "do");
    assert_eq!(
        service
            .execute(
                &format!("{path}/status"),
                &do_headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap()["status"],
        "queued"
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &mutation_caller(&binding),
                json!({"id":"forged","payloadBase64":"T0NEVgECAA==","definitionId":definition}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/restart"),
                &mutation_caller(&binding),
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap(),
        json!({"ok":true})
    );
    let mut stale = headers.clone();
    stale.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_static("bad"),
    );
    assert_eq!(
        service
            .execute(&format!("{path}/get"), &stale, json!({"id":"one"}), 11)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowBindingStale
    );

    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    let run = controller
        .claim(12, &mut Default::default())
        .unwrap()
        .unwrap();
    let declaration = |ordinal, name: &str, dependencies: Vec<u32>| {
        json!({
            "ordinal":ordinal,
            "kind":"do",
            "name":name,
            "nameCount":1,
            "config":{},
            "dependencies":dependencies,
            "batchFirstOrdinal":ordinal,
            "batchSize":1
        })
    };
    let first_claim = body(
        &run.fence,
        json!({"steps":[declaration(0,"lookup",vec![])],"remainingMs":config.dispatch_timeout_ms}),
    );
    let grant = service.run("claim-batch", first_claim.clone(), 13).unwrap();
    let first = &grant["steps"][0];
    assert_eq!(first["state"], "run");
    let success = body(
        &run.fence,
        json!({
            "ordinal":0,
            "attempt":first["attempt"],
            "stepToken":first["stepToken"],
            "outputBase64":"T0NEVgECEQAAAAEAAAAFdmFsdWUEQbPeQ1VVVVU="
        }),
    );
    assert_eq!(
        service.run("success", success.clone(), 14).unwrap()["state"],
        "complete"
    );
    assert_eq!(
        service.run("success", success, 15).unwrap_err().code(),
        ErrorCode::WorkflowStepStale
    );
    assert_eq!(
        service.run("claim-batch", first_claim, 15).unwrap()["steps"][0]["state"],
        "complete"
    );
    assert_eq!(
        service
            .run("result", body(&run.fence, json!({"ordinal":0})), 15)
            .unwrap()["outputBase64"],
        "T0NEVgECEQAAAAEAAAAFdmFsdWUEQbPeQ1VVVVU="
    );

    let second_claim = body(
        &run.fence,
        json!({"steps":[declaration(1,"fail",vec![0])],"remainingMs":config.dispatch_timeout_ms}),
    );
    let second = service
        .run("claim-batch", second_claim.clone(), 16)
        .unwrap();
    let second = &second["steps"][0];
    assert_eq!(
        service
            .run(
                "failure",
                body(
                    &run.fence,
                    json!({
                        "ordinal":1,
                        "attempt":second["attempt"],
                        "stepToken":second["stepToken"],
                        "error":{"name":"Error","message":"private-stack"}
                    }),
                ),
                17,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .run(
                "failure",
                body(
                    &run.fence,
                    json!({
                        "ordinal":1,
                        "attempt":second["attempt"],
                        "stepToken":second["stepToken"],
                        "code":"WORKFLOW_SERIALIZATION_UNSUPPORTED"
                    }),
                ),
                17,
            )
            .unwrap()["state"],
        "failed"
    );
    assert_eq!(
        service.run("claim-batch", second_claim, 18).unwrap()["steps"][0]["state"],
        "failed"
    );
    let failed = service
        .run("result", body(&run.fence, json!({"ordinal":1})), 18)
        .unwrap();
    assert_eq!(failed["code"], "WORKFLOW_SERIALIZATION_UNSUPPORTED");
    assert!(!failed.to_string().contains("private-stack"));
    f.scheduler
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "null".into(),
                final_ordinal: 2,
            },
            19,
            &config,
        )
        .unwrap();
    assert_eq!(
        f.scheduler
            .workflow_instance(run.fence.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Errored
    );
    assert_eq!(
        service
            .run(
                "claim-batch",
                body(
                    &run.fence,
                    json!({"steps":[declaration(2,"late",vec![1])],"remainingMs":config.dispatch_timeout_ms}),
                ),
                20,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let rendered = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(rendered.contains("open_compute_workflow_replay_steps_total{outcome=\"complete\"} 1"));
    assert!(rendered.contains("open_compute_workflow_replay_steps_total{outcome=\"failed\"} 1"));
    assert!(rendered.contains("open_compute_workflow_steps_total{outcome=\"success\"} 2"));
    assert!(rendered.contains("open_compute_workflow_steps_total{outcome=\"error\"} 1"));
    assert!(!rendered.contains("private-stack"));
}

#[test]
fn workflow_private_dynamic_delay_round_trip_is_durable() {
    let f = fixture();
    let (definition, _binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            WorkflowOperationId::generate(),
            Some("dynamic-delay"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            0,
        )
        .unwrap();
    let run = f
        .scheduler
        .claim_workflow(&identity, 0, &config)
        .unwrap()
        .unwrap();
    let service =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config.clone())
            .unwrap();
    let claim = service
        .run(
            "claim-batch",
            body(
                &run.fence,
                json!({"steps":[{
                    "ordinal":0,"kind":"do","name":"dynamic","nameCount":1,
                    "config":{"timeout":5,"retries":{"limit":1,"delay":{"dynamic":true}}},
                    "dependencies":[],"batchFirstOrdinal":0,"batchSize":1
                }],"remainingMs":config.dispatch_timeout_ms}),
            ),
            0,
        )
        .unwrap();
    let grant = &claim["steps"][0];
    let timeout = service
        .run(
            "timeout",
            body(
                &run.fence,
                json!({
                    "ordinal":0,
                    "attempt":grant["attempt"],
                    "stepToken":grant["stepToken"]
                }),
            ),
            5,
        )
        .unwrap();
    assert_eq!(timeout["state"], "resolve_delay");
    let resolved = service
        .run(
            "resolve-delay",
            body(
                &run.fence,
                json!({
                    "ordinal":0,"attempt":1,"code":"WORKFLOW_STEP_TIMEOUT",
                    "resolvedDelayMs":0
                }),
            ),
            5,
        )
        .unwrap();
    assert_eq!(resolved["state"], "suspended");
    assert_eq!(
        service
            .run("yield", body(&run.fence, json!({"finalOrdinal":1025})), 5,)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepLimitExceeded
    );
    assert_eq!(
        service
            .run("unknown", body(&run.fence, json!({})), 5)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
}

#[test]
fn workflow_private_protocol_rejects_shape_drift_and_persists_failure_delay() {
    let f = fixture();
    let (definition, _binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            WorkflowOperationId::generate(),
            Some("private-failure-delay"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            0,
        )
        .unwrap();
    let run = f
        .scheduler
        .claim_workflow(&identity, 0, &config)
        .unwrap()
        .unwrap();
    let service =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config.clone())
            .unwrap();
    let claim = service
        .run(
            "claim-batch",
            body(
                &run.fence,
                json!({"steps":[{
                    "ordinal":0,"kind":"do","name":"retry","nameCount":1,
                    "config":{"timeout":10,"retries":{"limit":1,"delay":{"dynamic":true}}},
                    "dependencies":[],"batchFirstOrdinal":0,"batchSize":1
                }],"remainingMs":config.dispatch_timeout_ms}),
            ),
            0,
        )
        .unwrap();
    let grant = &claim["steps"][0];

    assert_eq!(
        service
            .run(
                "timeout",
                body(
                    &run.fence,
                    json!({
                        "ordinal":0,"attempt":grant["attempt"],
                        "stepToken":grant["stepToken"],"extra":true
                    }),
                ),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    assert_eq!(
        service
            .run(
                "success",
                body(&run.fence, json!({"ordinal":0,"attempt":1})),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .run("result", json!("not-an-object"), 1)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let settled = service
        .run(
            "failure",
            body(
                &run.fence,
                json!({
                    "ordinal":0,"attempt":grant["attempt"],"stepToken":grant["stepToken"],
                    "code":"WORKFLOW_STEP_TIMEOUT","resolvedDelayMs":3
                }),
            ),
            1,
        )
        .unwrap();
    assert_eq!(settled["state"], "suspended");
}

#[tokio::test]
async fn workflow_private_http_is_bounded_and_rechecks_startup_generation() {
    let f = fixture();
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig {
            max_in_flight_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("ab".repeat(32)));
    let request = |content: &str, body: axum::body::Body| {
        Request::builder()
            .method("POST")
            .uri("/internal/workflows/runs/claim-batch")
            .header("content-type", content)
            .header("x-open-compute-binding-token", "ab".repeat(32))
            .header("x-open-compute-startup-generation", "generation-one")
            .body(body)
            .unwrap()
    };
    let response = service
        .handle(
            request("text/plain", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_METHOD_UNSUPPORTED"
    );
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("not json")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_SERIALIZATION_UNSUPPORTED"
    );
    let response = service
        .handle(
            request(
                "application/json",
                axum::body::Body::from("x".repeat(MAX_BODY + 1)),
            ),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let permit = service.concurrency.clone().acquire_owned().await.unwrap();
    let response = service
        .handle(
            request("application/json", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(permit);
    auth.activate_for_test(SecretString::new("cd".repeat(32)));
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("{}")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_RUN_STALE"
    );
    assert!(
        to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    for (code, status) in [
        (ErrorCode::WorkflowRuntimeUnavailable, 503),
        (ErrorCode::WorkflowStateQuotaExceeded, 429),
        (ErrorCode::WorkflowEventQueueFull, 429),
        (ErrorCode::WorkflowInstanceNotFound, 404),
        (ErrorCode::WorkflowRunStale, 409),
        (ErrorCode::WorkflowInstanceBusy, 409),
        (ErrorCode::WorkflowInstanceStateConflict, 409),
        (ErrorCode::WorkflowInstanceCleanupPending, 409),
        (ErrorCode::WorkflowResultTooLarge, 413),
        (ErrorCode::WorkflowSerializationUnsupported, 422),
    ] {
        assert_eq!(response_error(code).status().as_u16(), status);
    }
}

#[test]
fn workflow_metric_guards_count_all_outcomes_without_sensitive_labels() {
    let f = fixture();
    for outcome in [
        WorkflowOutcome::Success,
        WorkflowOutcome::Error,
        WorkflowOutcome::Unknown,
    ] {
        let mut guard = f.metrics.workflow_run();
        guard.finish(outcome);
        f.metrics.workflow_created(outcome);
        f.metrics.workflow_step(outcome, Duration::from_millis(5));
    }
    f.metrics.workflow_reconcile(true);
    f.metrics.workflow_reconcile(false);
    f.metrics.workflow_stale(true);
    f.metrics.workflow_stale(false);
    f.metrics.workflow_summary(
        &open_compute_storage::scheduler::WorkflowInspection {
            queued: 1,
            running: 2,
            complete: 3,
            errored: 4,
            state_bytes: 100,
            expired_runs: 1,
            waiting: 5,
            paused: 6,
            terminated: 7,
            retained: 8,
            buffered_events: 2,
            inbox_bytes: 64,
            consumed_events: 3,
            sleeping_steps: 2,
            event_waits: 1,
            retry_waits: 4,
            retried_steps: 3,
            exhausted_steps: 1,
            step_timeouts: 1,
            event_timeouts: 2,
            gc_receipts: 1,
        },
        0.5,
    );
    f.metrics.workflow_operations(
        &open_compute_storage::WorkflowOperationInspection {
            pending_restarts: 1,
            pending_purges: 2,
            oldest_operation_at_ms: Some(1000),
        },
        2500,
    );
    for failure in [
        None,
        Some(ErrorCode::WorkflowEventQueueFull),
        Some(ErrorCode::WorkflowInstanceBusy),
    ] {
        f.metrics.workflow_event(failure);
    }
    for operation in ["pause", "resume", "terminate", "restart", "private-label"] {
        f.metrics.workflow_lifecycle(operation, true);
        f.metrics.workflow_lifecycle(operation, false);
    }
    let output = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(output.contains("open_compute_workflow_in_flight 0"));
    assert!(output.contains("open_compute_workflow_runs_total{outcome=\"unknown\"} 1"));
    assert!(output.contains("open_compute_workflow_instance_status{status=\"complete\"} 3"));
    for line in [
        "open_compute_workflow_instance_status{status=\"paused\"} 6",
        "open_compute_workflow_instance_status{status=\"running\"} 2",
        "open_compute_workflow_waiting_steps{reason=\"retry\"} 4",
        "open_compute_workflow_pending_operations{phase=\"purge_receipt\"} 1",
        "open_compute_workflow_event_intake_total{outcome=\"full\"} 1",
        "open_compute_workflow_lifecycle_total{operation=\"restart\",outcome=\"error\"} 1",
        "open_compute_workflow_operation_age_seconds 1.5",
        "open_compute_workflow_consumed_events 3",
    ] {
        assert!(output.contains(line), "missing {line}");
    }
    assert!(!output.contains("private-label"));
}
