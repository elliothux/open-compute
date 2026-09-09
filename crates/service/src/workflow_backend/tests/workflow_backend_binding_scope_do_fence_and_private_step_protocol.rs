use super::*;

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
