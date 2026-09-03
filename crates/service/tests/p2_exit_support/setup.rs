//! Provision real product resources before transferring exclusive ownership to ocd.

use crate::p0_exit_support::{
    GateStack, admin_json, admin_router, deploy, now_ms, open_scheduler, repo_root, storage_config,
    stores, wait_pid_change,
};
use crate::platform_process::Evidence;
use open_compute_artifacts::MockS3;
use open_compute_core::{
    AccountId, BindingKind, QueueId, RequestId, ResourceId, SystemClock, VersionId, WorkflowId,
};
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_storage::{PlatformStorage, WorkerRepository, WorkflowRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateQueueOutcome, CreateQueueRequest, CreateVersionRequest,
    ModuleInput, ModuleType, QueueConsumerInput, QueueController, ResourcePins,
    VersionBindingInput, VersionController,
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

pub(super) struct Fixture {
    pub evidence: Evidence,
    pub data: PathBuf,
    pub mock: MockS3,
    pub account: AccountId,
    pub queue: QueueId,
    pub definition: WorkflowId,
    pub frozen: VersionId,
    pub future: VersionId,
}

pub(super) async fn prepare() -> Fixture {
    let root = repo_root();
    std::fs::create_dir_all(root.join(".temp/p2-exit-run")).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("chain-")
        .tempdir_in(root.join(".temp/p2-exit-run"))
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
        root.join("packages/runtime/workerd.lock.json"),
        root.join("packages/runtime"),
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
        .create_worker(
            account,
            "p2-chain",
            RequestId::generate(),
            now_ms(),
            1_000_000,
        )
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
    ] {
        let (status, result) = admin_json(
            &api,
            "POST",
            &format!("/operator/api/v1/accounts/{account}/{path}"),
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
            binding(kind, id.as_str().unwrap().parse().unwrap()),
        );
    }
    let do_repository = open_compute_storage::DurableObjectRepository::new(&storage);
    let do_plan = open_compute_storage::DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "p2-chain-v1".to_owned(),
        new_sqlite_classes: vec!["Counter".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    do_repository
        .prepare_worker_migration(account, worker.id, &do_plan, now_ms())
        .unwrap();
    let namespace = do_repository
        .namespace_for_worker_upload(account, worker.id, "Counter", Some("p2-chain-v1"))
        .unwrap();
    bindings.insert(
        "OBJECTS".into(),
        binding(BindingKind::DoNamespace, namespace.resource.id),
    );
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
    let source = include_str!("../fixtures/p2-exit-worker.js");
    let mut versions = Vec::new();
    for index in 0..3 {
        let mut bound = bindings.clone();
        if index == 1 {
            bound.insert(
                "FLOW".into(),
                binding(
                    BindingKind::Workflow,
                    ResourceId::from_uuid(definition.as_uuid()).unwrap(),
                ),
            );
            bound.insert(
                "QUEUE".into(),
                binding(
                    BindingKind::QueueProducer,
                    ResourceId::from_uuid(queue.as_uuid()).unwrap(),
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
        let request = CreateVersionRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: format!("chain-{index}"),
            content: open_compute_workers::VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: Default::default(),
            secrets: Default::default(),
            bindings: bound,
            services: Default::default(),
            runtime_features: Default::default(),
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
            crons: Vec::new(),
            deployment_source: (index != 2)
                .then_some(open_compute_storage::DeploymentSource::VersionsApi),
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        };
        let mut controller = VersionController::new(
            &storage,
            artifacts.clone(),
            Arc::new(stack.transport.clone()),
            BundleLimits::default(),
        )
        .with_product_promoter(open_compute_service::product_promotion_for_test(
            storage.clone(),
            scheduler.clone(),
        ));
        if index == 0 {
            controller = controller.with_durable_object_migration(do_plan.clone());
        }
        let version = deploy(&controller, request, &stack.supervisor).await;
        // The first version makes the self-binding deployable; publish the
        // gateway's version too so its DO calls use the active Worker version.
        // Later the test advances only the Workflow version, preserving the
        // existing DO contract that rejects retired Worker versions.
        if index < 2 {
            let version = WorkflowApiState::new(
                storage.clone(),
                scheduler.clone(),
                stack.transport.clone(),
                Default::default(),
            )
            .create_version(account, definition, version.id, "Flow".into())
            .await
            .unwrap();
            if version.state != open_compute_storage::VersionState::Ready {
                let failed_pid = stack.supervisor.snapshot().pid.unwrap();
                stack.supervisor.report_unhealthy();
                wait_pid_change(&stack.supervisor, failed_pid, Duration::from_secs(30)).await;
                panic!(
                    "workflow version {index} did not validate: {:?}; diagnostics={:?}",
                    version.state,
                    stack.supervisor.last_diagnostics(),
                );
            }
        }
        versions.push(version.id);
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
            1_000_000,
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
        frozen: versions[1],
        future: versions[2],
    }
}

fn binding(kind: BindingKind, id: ResourceId) -> VersionBindingInput {
    VersionBindingInput {
        kind,
        id,
        permissions: Default::default(),
        config: Default::default(),
    }
}
