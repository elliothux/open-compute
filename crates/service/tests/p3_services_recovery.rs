//! Real SIGKILL cleanup for in-flight Service handles and deployment pins.

#![cfg(feature = "test-support")]

mod p3_services_support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use futures::StreamExt;
use open_compute_core::RequestId;
use open_compute_runtime::SupervisorState;
use open_compute_service::runtime_bridge::DispatchTarget;
use open_compute_storage::WorkerRepository;
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentContent, DeploymentController, DeploymentServiceInput, ModuleInput, ModuleType,
    RuntimeValidator,
};
use p3_services_support::Harness;
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TARGET: &str = r#"
import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
class Held extends RpcTarget {
  ping(value) { return `held:${value}`; }
}
export default class Target extends WorkerEntrypoint {
  capability() { return new Held(); }
  ping() { return "pong"; }
}
"#;

const CALLER: &str = r#"
import { WorkerEntrypoint } from "cloudflare:workers";
export default class Caller extends WorkerEntrypoint {
  async fetch(request) {
    if (new URL(request.url).pathname === "/ping") {
      return new Response(await this.env.TARGET.ping());
    }
    const target = this.env.TARGET;
    return new Response(new ReadableStream({
      async start(controller) {
        const held = await target.capability();
        controller.enqueue(new TextEncoder().encode("ready\n"));
        await scheduler.wait(30000);
        controller.enqueue(new TextEncoder().encode(await held.ping("late")));
        held[Symbol.dispose]();
        controller.close();
      },
    }));
  }
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_service_generation_exit_releases_inflight_handles_and_pins() {
    let harness = Harness::start("p3-services-recovery").await;
    let account = harness.storage.identity().default_account_id;
    let repository = WorkerRepository::new(harness.storage.db());
    let (target, _) = repository
        .create_worker(
            account,
            "service-crash-target",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let (caller, _) = repository
        .create_worker(
            account,
            "service-crash-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(harness.transport.clone());
    let controller = DeploymentController::new(
        &harness.storage,
        harness.artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let target_deployment = deploy(
        &controller,
        deployment_request(
            account,
            target.id,
            "service-crash-target-v1",
            TARGET,
            BTreeMap::new(),
            1,
        ),
    )
    .await;
    let caller_deployment = deploy(
        &controller,
        deployment_request(
            account,
            caller.id,
            "service-crash-caller-v1",
            CALLER,
            BTreeMap::from([(
                "TARGET".to_owned(),
                DeploymentServiceInput {
                    target_worker_id: target.id,
                    entrypoint: None,
                },
            )]),
            2,
        ),
    )
    .await;

    let response = dispatch(&harness, caller.id, &caller_deployment, "/hold").await;
    assert_eq!(response.status(), 200);
    let mut stream = response.into_body().into_data_stream();
    let ready = stream.next().await.unwrap().unwrap();
    assert_eq!(ready.as_ref(), b"ready\n");
    wait_for(
        Duration::from_secs(5),
        "in-flight Service retention",
        || {
            let counts = harness.service_invocations.counts();
            counts.0 == 1
                && counts.2 == 1
                && harness.deployment_pins.count(target_deployment.id) == 1
        },
    )
    .await;

    let old_pid = harness.supervisor.snapshot().pid.unwrap();
    kill_workerd(old_pid);
    wait_for(
        Duration::from_secs(10),
        "generation resource cleanup",
        || {
            harness.service_invocations.counts() == (0, 0, 0)
                && harness.deployment_pins.count(target_deployment.id) == 0
        },
    )
    .await;
    wait_for(Duration::from_secs(20), "replacement workerd", || {
        let snapshot = harness.supervisor.snapshot();
        snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid)
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while stream.next().await.is_some() {}
    })
    .await;

    let response = dispatch(&harness, caller.id, &caller_deployment, "/ping").await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        b"pong"
    );
    wait_for(
        Duration::from_secs(5),
        "fresh-generation Service drain",
        || {
            harness.service_invocations.counts() == (0, 0, 0)
                && harness.deployment_pins.count(target_deployment.id) == 0
        },
    )
    .await;
    harness.stop().await;
}

fn deployment_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    source: &str,
    services: BTreeMap<String, DeploymentServiceInput>,
    now_ms: i64,
) -> CreateDeploymentRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
) -> open_compute_storage::DeploymentRecord {
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

async fn dispatch(
    harness: &Harness,
    worker_id: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    path: &str,
) -> axum::response::Response {
    harness
        .transport
        .dispatch(
            DispatchTarget {
                account_id: harness.storage.identity().default_account_id,
                worker_id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: None,
                route_generation: 1,
                request_id: RequestId::generate(),
            },
            Request::builder()
                .method("GET")
                .uri(path)
                .header(header::HOST, "service-crash.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn kill_workerd(pid: i32) {
    let status = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "failed to SIGKILL workerd {pid}");
}

async fn wait_for(timeout: Duration, label: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "{label} did not become true");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
