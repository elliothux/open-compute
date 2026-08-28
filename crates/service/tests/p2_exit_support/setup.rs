//! Provision real product resources before transferring exclusive ownership to platformd.

use crate::p0_exit_support::{
    GateStack, admin_json, admin_router, deploy, now_ms, open_scheduler, repo_root, storage_config,
    stores,
};
use crate::platform_process::Evidence;
use open_compute_artifacts::MockS3;
use open_compute_core::{
    AccountId, BindingKind, DeploymentId, QueueId, RequestId, ResourceId, SystemClock, WorkflowId,
};
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_storage::{PlatformStorage, WorkerRepository, WorkflowRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentRequest, CreateQueueOutcome, CreateQueueRequest,
    DeploymentBindingInput, DeploymentController, ModuleInput, ModuleType, QueueConsumerInput,
    QueueController, ResourcePins,
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(super) struct Fixture {
    pub evidence: Evidence,
    pub data: PathBuf,
    pub mock: MockS3,
    pub account: AccountId,
    pub queue: QueueId,
    pub definition: WorkflowId,
    pub frozen: DeploymentId,
    pub future: DeploymentId,
}

pub(super) async fn prepare() -> Fixture {
    let root = repo_root();
    std::fs::create_dir_all(root.join(".p2-exit-run")).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("chain-")
        .tempdir_in(root.join(".p2-exit-run"))
        .unwrap();
    let evidence = Evidence(Some(temp));
    let data = evidence.0.as_ref().unwrap().path().join("data");
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&data), &SystemClock).unwrap());
    let scheduler = open_scheduler(&storage);
    let mock = MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
            .unwrap()
            .into(),
        root.join("runtime/workerd.lock.json"),
        root.join("runtime"),
        "p2-exit",
    )
    .await;
    let api = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects,
        pins,
        &stack,
        scheduler.clone(),
    );
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "p2-chain", RequestId::generate(), now_ms())
        .unwrap()
        .0;
    let mut bindings = BTreeMap::new();
    for (name, kind, path, body, nested) in [
        (
            "KV",
            BindingKind::KvNamespace,
            "kv/namespaces",
            json!({"name":"chain-kv"}),
            false,
        ),
        (
            "R2",
            BindingKind::R2Bucket,
            "r2/buckets",
            json!({"name":"chain-r2"}),
            true,
        ),
        (
            "DB",
            BindingKind::D1Database,
            "d1/databases",
            json!({"name":"chain-db"}),
            false,
        ),
        (
            "OBJECTS",
            BindingKind::DoNamespace,
            "durable-objects/namespaces",
            json!({"name":"chain-objects","workerId":worker.id,"className":"Counter"}),
            false,
        ),
    ] {
        let (status, result) = admin_json(
            &api,
            "POST",
            &format!("/v1/accounts/{account}/{path}"),
            body,
            Some(name),
        )
        .await;
        assert!(status.is_success(), "{result}");
        let id = if nested {
            &result["bucket"]["resourceId"]
        } else {
            &result["resourceId"]
        };
        bindings.insert(
            name.into(),
            binding(kind, id.as_str().unwrap().parse().unwrap(), 1),
        );
    }
    let CreateQueueOutcome::Applied(queue) = QueueController::new(&storage, scheduler.clone())
        .create(&CreateQueueRequest {
            account_id: account,
            name: "chain-queue".into(),
            config: Default::default(),
            idempotency_key: "chain-queue".into(),
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        })
        .unwrap()
    else {
        panic!("queue create must apply")
    };
    let queue = queue.queue.id;
    let definition = WorkflowRepository::new(storage.db())
        .create_definition(account, "chain-flow", now_ms())
        .unwrap()
        .id;
    let controller = DeploymentController::new(
        &storage,
        artifacts,
        Arc::new(stack.transport.clone()),
        BundleLimits::default(),
    )
    .with_product_promoter(open_compute_service::product_promotion_for_test(
        storage.clone(),
        scheduler.clone(),
    ));
    let source = include_str!("../fixtures/p2-exit-worker.js");
    let mut deployments = Vec::new();
    for index in 0..3 {
        let mut bound = bindings.clone();
        if index == 1 {
            bound.insert(
                "FLOW".into(),
                binding(
                    BindingKind::Workflow,
                    ResourceId::from_uuid(definition.as_uuid()).unwrap(),
                    2,
                ),
            );
            bound.insert(
                "QUEUE".into(),
                binding(
                    BindingKind::QueueProducer,
                    ResourceId::from_uuid(queue.as_uuid()).unwrap(),
                    1,
                ),
            );
        }
        let source = if index == 2 {
            source.replace("const VERSION = 'frozen'", "const VERSION = 'future'")
        } else {
            source.into()
        };
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".into(),
                module_type: ModuleType::EsModule,
                bytes: source.into_bytes(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        let request = CreateDeploymentRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: format!("chain-{index}"),
            bundle: bundle.into_bytes().into(),
            compatibility_date: "2026-08-22".into(),
            compatibility_flags: vec!["rpc".into()],
            vars: Default::default(),
            secrets: Default::default(),
            bindings: bound,
            queue_consumers: if index == 1 {
                vec![QueueConsumerInput {
                    queue,
                    entrypoint: None,
                    config: Default::default(),
                    dead_letter_queue: None,
                }]
            } else {
                vec![]
            },
            crons: None,
            limits: json!({"profile":"default"}),
            promote: index != 2,
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        };
        let deployment = deploy(&controller, request, &stack.supervisor).await;
        // The first version makes the self-binding deployable; publish the
        // gateway's version too so its DO calls use the active Worker deployment.
        // Later the test advances only the Workflow version, preserving the
        // existing DO contract that rejects retired Worker deployments.
        if index < 2 {
            let version = WorkflowApiState::new(
                storage.clone(),
                scheduler.clone(),
                stack.transport.clone(),
                Default::default(),
            )
            .create_version(account, definition, deployment.id, "Flow".into(), 2)
            .await
            .unwrap();
            assert_eq!(version.state, open_compute_storage::DeploymentState::Ready);
        }
        deployments.push(deployment.id);
    }
    workers
        .create_exact_route(
            account,
            worker.id,
            "workflow.example",
            "/",
            None,
            None,
            RequestId::generate(),
            now_ms(),
        )
        .unwrap();
    stack.stop().await;
    Fixture {
        evidence,
        data,
        mock,
        account,
        queue,
        definition,
        frozen: deployments[1],
        future: deployments[2],
    }
}

fn binding(kind: BindingKind, id: ResourceId, capability_version: u32) -> DeploymentBindingInput {
    DeploymentBindingInput {
        kind,
        id,
        capability_version,
        permissions: Default::default(),
        config: Default::default(),
    }
}
