//! Real pinned-workerd P0.7 Durable Objects identity, facet, lifecycle, and restart Gate.
//!
//! This intentionally stays one cohesive process matrix so one fixture proves identity,
//! version fencing, native persistence, `WebSockets`, and destructive lifecycle together.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use hmac::{Hmac, Mac};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, DurableObjectsConfig, PlatformConfig, RuntimeConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DurableObjectId,
    Redactor, RequestId, ResourceId, WorkerId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository, PlatformStorage, VersionRecord,
    WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeSource, RuntimeValidator,
    VersionBindingInput, VersionContent, VersionController,
};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "../../../test/runtime/durable-objects/hibernation.rs"]
mod hibernation;
#[path = "../../../test/runtime/durable-objects/output_crash.rs"]
mod output_crash;
#[path = "../../../test/runtime/durable-objects/recovery.rs"]
mod recovery;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_7_real_durable_objects_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler = output_crash::open_scheduler(&storage);
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let resource_pins = ResourcePins::new();
    let (shutdown_tx, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let backend_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = resource_pins.clone();
        let scheduler = scheduler.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                backend_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                durable_objects_config(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                Some(scheduler),
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.7-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
        .with_max_request_body(32 * 1024 * 1024);
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-7-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let (output_queue, output_queue_resource) =
        output_crash::create_queue(&storage, scheduler.clone(), account);
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "do-matrix", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let counter = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "Counter",
        "counter",
        11,
    );
    let other = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "OtherCounter",
        "other",
        12,
    );
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());
    let version_a = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            counter,
            other,
            output_queue_resource,
            "deploy-a",
            "A",
            20,
            true,
        ),
        &supervisor,
    )
    .await;
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;

    let ids = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/ids",
    )
    .await;
    assert_eq!(ids.status, 200, "{}", ids.body);
    let identity: serde_json::Value = serde_json::from_str(&ids.body).unwrap();
    let named_id = identity["named"].as_str().unwrap();
    assert_eq!(named_id.len(), 64);
    assert_eq!(identity["named"], identity["namedAgain"]);
    assert_ne!(identity["named"], identity["unique"]);
    assert_eq!(identity["crossNamespaceRejected"], true);
    assert_eq!(identity["uppercaseRejected"], true);
    assert_eq!(identity["invalidHintRejected"], true);
    assert_eq!(identity["locationAccepted"], true);
    assert_eq!(identity["jurisdiction"], "eu");
    assert_eq!(identity["namedJurisdiction"], "eu");
    assert_eq!(identity["jurisdictionRoundTrip"], true);
    assert_eq!(identity["jurisdictionChangesId"], true);
    assert_eq!(identity["unscopedGetAcceptsJurisdiction"], true);
    assert_eq!(identity["nullishJurisdiction"], true);
    assert_eq!(identity["forgedRejected"], true);
    assert_eq!(identity["forgedBridgeRejected"], true);
    assert_eq!(identity["mutatedIntrinsicNamed"], identity["named"]);
    assert!(
        DurableObjectId::from_str(named_id)
            .unwrap()
            .belongs_to(counter)
    );
    let (prefix, name_key) = DurableObjectRepository::new(&storage)
        .facade_identity(counter)
        .unwrap();
    let mut expected = Vec::from(prefix);
    let mut mac = <Hmac<Sha256>>::new_from_slice(&name_key).unwrap();
    mac.update(b"\x6e\x00alpha");
    let named_body = mac.finalize().into_bytes();
    let mut payload = Vec::with_capacity(24);
    payload.push(0xa0);
    payload.extend_from_slice(&named_body[..15]);
    let mut tag = <Hmac<Sha256>>::new_from_slice(&name_key).unwrap();
    tag.update(&payload);
    payload.extend_from_slice(&tag.finalize().into_bytes()[..8]);
    expected.extend_from_slice(&payload);
    assert_eq!(named_id, hex::encode(expected));

    let first = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    if first.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "first DO dispatch failed: {}; diagnostics={:?}",
            first.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(first.body, "A:1");
    let second = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc?name=alpha",
    )
    .await;
    if second.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO RPC failed: {}; diagnostics={:?}",
            second.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(second.body, "A:1");
    let binary_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-binary?name=alpha",
    )
    .await;
    assert_eq!(
        (binary_rpc.status, binary_rpc.body.as_str()),
        (200, "4,5,6")
    );
    let connect = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/connect?name=alpha",
    )
    .await;
    assert_eq!(
        (connect.status, connect.body.as_str()),
        (200, "4,5,6"),
        "diagnostics={:?}",
        supervisor.last_diagnostics()
    );
    let connect_ipv6 = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/connect-ipv6?name=alpha",
    )
    .await;
    assert_eq!(
        (connect_ipv6.status, connect_ipv6.body.as_str()),
        (200, "10,11,12"),
        "diagnostics={:?}",
        supervisor.last_diagnostics()
    );
    let structured_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-structured?name=alpha",
    )
    .await;
    assert_eq!(structured_rpc.status, 200, "{}", structured_rpc.body);
    let structured: serde_json::Value = serde_json::from_str(&structured_rpc.body).unwrap();
    assert_eq!(structured["time"], "2026-08-30T00:00:00.000Z");
    for member in [
        "bigint", "map", "regexp", "error", "typed", "view", "buffer", "headers", "request",
        "response",
    ] {
        assert_eq!(structured[member], true, "{member}: {structured}");
    }
    let stream_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-stream?name=alpha",
    )
    .await;
    assert_eq!(
        (stream_rpc.status, stream_rpc.body.as_str()),
        (200, "7,8,9")
    );
    let writable_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-writable?name=alpha",
    )
    .await;
    assert_eq!(
        (writable_rpc.status, writable_rpc.body.as_str()),
        (200, "10,11")
    );
    let capability_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&capability_rpc, "A");
    let property_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-property?name=alpha",
    )
    .await;
    assert_eq!(property_rpc.status, 200, "{}", property_rpc.body);
    let property: serde_json::Value = serde_json::from_str(&property_rpc.body).unwrap();
    assert_eq!(property["regular"], "A:property");
    assert_eq!(property["punctuation"], "A:punctuation");
    assert_eq!(property["method"], "A:punctuation-method");
    let property_error = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-property-error?name=alpha",
    )
    .await;
    assert_eq!(
        (property_error.status, property_error.body.as_str()),
        (200, "true")
    );
    let callback_rpc = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-callback?name=alpha",
    )
    .await;
    assert_eq!(callback_rpc.status, 200, "{}", callback_rpc.body);
    let callback: serde_json::Value = serde_json::from_str(&callback_rpc.body).unwrap();
    assert_eq!(callback["target"], "target:ok");
    assert_eq!(callback["callback"], "function:ok");
    let clone_error = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-clone-error?name=alpha",
    )
    .await;
    assert_eq!(
        (clone_error.status, clone_error.body.as_str()),
        (200, "true")
    );
    let capability_error = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-capability-error?name=alpha",
    )
    .await;
    assert_eq!(
        (capability_error.status, capability_error.body.as_str()),
        (200, "true"),
        "tenant RpcTarget exceptions must be sanitized at the trust boundary"
    );
    let rpc_error = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rpc-error?name=alpha",
    )
    .await;
    assert_eq!((rpc_error.status, rpc_error.body.as_str()), (200, "true"));
    let rollback = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/rollback?name=alpha",
    )
    .await;
    assert_eq!((rollback.status, rollback.body.as_str()), (200, "true:1"));
    let websocket = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/websocket?name=alpha",
    )
    .await;
    if websocket.status != 200 {
        let failed_pid = supervisor.snapshot().pid.unwrap();
        supervisor.report_unhealthy();
        wait_pid_change(&supervisor, failed_pid, Duration::from_secs(30)).await;
        panic!(
            "DO websocket failed: {}; diagnostics={:?}",
            websocket.body,
            supervisor.last_diagnostics()
        );
    }
    assert_eq!(websocket.body, "text:true,binary:true");

    let ordered = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/order?name=ordered",
    )
    .await;
    assert_eq!(ordered.status, 200, "{}", ordered.body);
    let order: Vec<String> = serde_json::from_str(&ordered.body).unwrap();
    let first_start = order.iter().position(|item| item == "first:start").unwrap();
    let second_start = order
        .iter()
        .position(|item| item == "second:start")
        .unwrap();
    assert!(first_start < second_start, "same-stub E-order: {order:?}");

    let cross_ordered = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/cross-order?name=cross-ordered",
    )
    .await;
    assert_eq!(cross_ordered.status, 200, "{}", cross_ordered.body);
    let cross_order: serde_json::Value = serde_json::from_str(&cross_ordered.body).unwrap();
    assert_eq!(cross_order["echoed"], true, "{cross_order}");
    let starts = cross_order["order"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .filter(|value| value.ends_with(":start"))
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        [
            "rpc-first:start",
            "fetch-second:start",
            "connect:start",
            "rpc-fourth:start",
        ],
        "cross-surface same-stub E-order: {cross_order}"
    );

    let order_error = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/order-error?name=order-error",
    )
    .await;
    assert_eq!(order_error.status, 200, "{}", order_error.body);
    let order_error: serde_json::Value = serde_json::from_str(&order_error.body).unwrap();
    for key in ["failed", "fetched", "rpc"] {
        assert_eq!(order_error[key], true, "{key}: {order_error}");
    }

    let storage_matrix = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/storage?name=storage-matrix",
    )
    .await;
    assert_eq!(storage_matrix.status, 200, "{}", storage_matrix.body);
    hibernation::storage_members(&serde_json::from_str(&storage_matrix.body).unwrap());
    hibernation::facets(&transport, account, worker.id, &version_a, generation_a).await;

    let parallel_start = Instant::now();
    let (left, right) = tokio::join!(
        dispatch(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/hold?name=left&ms=250&window=1",
        ),
        dispatch(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/hold?name=right&ms=250&window=1",
        ),
    );
    assert_eq!(
        (left.status, right.status),
        (200, 200),
        "left={} right={}",
        left.body,
        right.body
    );
    let left_hold: serde_json::Value = serde_json::from_str(&left.body).expect("left hold json");
    let right_hold: serde_json::Value = serde_json::from_str(&right.body).expect("right hold json");
    let left_t0 = left_hold["t0"].as_i64().expect("left t0");
    let left_t1 = left_hold["t1"].as_i64().expect("left t1");
    let right_t0 = right_hold["t0"].as_i64().expect("right t0");
    let right_t1 = right_hold["t1"].as_i64().expect("right t1");
    assert!(
        left_t0 < right_t1 && right_t0 < left_t1,
        "parallel DO holds must overlap: left={left_hold} right={right_hold} wall={:?}",
        parallel_start.elapsed(),
    );

    let mut missing_class = version_request(
        account,
        worker.id,
        counter,
        other,
        output_queue_resource,
        "missing-class",
        "invalid",
        29,
        false,
    );
    missing_class.content = VersionContent::Worker {
        bundle: CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: b"export default { fetch() { return new Response('missing'); } };".to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap()
        .into_bytes()
        .into(),
        assets: None,
    };
    assert_eq!(
        versions
            .create_version(missing_class)
            .await
            .unwrap_err()
            .code(),
        open_compute_core::ErrorCode::DoClassNotFound
    );

    let in_flight = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                generation_a,
                "/hold?name=alpha&ms=3000",
            )
            .await
        }
    });
    let capability_in_flight = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                generation_a,
                "/rpc-pipeline-hold?name=alpha&ms=3000",
            )
            .await
        }
    });
    let admitted_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let admitted = dispatch(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/hold-started?name=alpha",
        )
        .await;
        assert_eq!(admitted.status, 200, "{}", admitted.body);
        let admitted: serde_json::Value = serde_json::from_str(&admitted.body).unwrap();
        if admitted["fetch"] == true && admitted["capability"] == true {
            break;
        }
        assert!(
            Instant::now() < admitted_deadline,
            "old-generation operations were not admitted before promotion: {admitted}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let version_b = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            counter,
            other,
            output_queue_resource,
            "deploy-b",
            "B",
            30,
            true,
        ),
        &supervisor,
    )
    .await;
    let completed_in_flight = in_flight.await.unwrap();
    assert_eq!(
        (
            completed_in_flight.status,
            completed_in_flight.body.as_str()
        ),
        (200, "A:2")
    );
    let completed_capability = capability_in_flight.await.unwrap();
    assert_eq!(
        (
            completed_capability.status,
            completed_capability.body.as_str()
        ),
        (200, "A:ok"),
        "an admitted old-generation RPC capability must remain pinned until its request completes"
    );
    let generation_b = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    let promoted = dispatch(
        &transport,
        account,
        worker.id,
        &version_b,
        generation_b,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((promoted.status, promoted.body.as_str()), (200, "B:3"));
    let promoted_capability = dispatch(
        &transport,
        account,
        worker.id,
        &version_b,
        generation_b,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&promoted_capability, "B");
    let stale = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!(stale.status, 500);

    workers
        .promote_checked(
            account,
            worker.id,
            version_a.id,
            Some(version_b.id),
            Some(generation_b),
            RequestId::generate(),
            40,
        )
        .unwrap();
    let generation_rollback = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    let rolled = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((rolled.status, rolled.body.as_str()), (200, "A:3"));

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let recovered = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/rpc?name=alpha",
    )
    .await;
    assert_eq!((recovered.status, recovered.body.as_str()), (200, "A:3"));
    let capability_after_restart = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&capability_after_restart, "A");

    let pending_capability = tokio::spawn({
        let transport = transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                generation_rollback,
                "/rpc-pipeline-hold?name=alpha&ms=60000",
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let old_pid = supervisor.snapshot().pid.unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(old_pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    match tokio::time::timeout(Duration::from_secs(10), pending_capability).await {
        Ok(Ok(response)) => assert_ne!(
            (response.status, response.body.as_str()),
            (200, "A:ok"),
            "an RPC capability from a dead runtime generation remained callable"
        ),
        Ok(Err(_)) => {}
        Err(_) => panic!("dead runtime-generation RPC capability did not settle"),
    }
    let fresh_capability = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/rpc-capability?name=alpha",
    )
    .await;
    assert_rpc_capability(&fresh_capability, "A");

    recovery::check(
        &transport,
        &supervisor,
        account,
        worker.id,
        &version_a,
        generation_rollback,
    )
    .await;
    hibernation::check(
        &transport,
        &supervisor,
        account,
        worker.id,
        &version_a,
        generation_rollback,
    )
    .await;
    output_crash::check(
        &transport,
        &supervisor,
        &scheduler,
        output_crash::Target {
            queue: output_queue,
            account,
            worker: worker.id,
            version: &version_a,
            generation: generation_rollback,
        },
    )
    .await;

    let object_id = DurableObjectId::from_str(named_id).unwrap();
    let repository = DurableObjectRepository::new(&storage);
    let fenced = repository
        .begin_object_delete(account, counter, object_id, 50)
        .unwrap();
    let authority = repository
        .deletion_authority(account, counter, object_id, fenced.generation)
        .unwrap();
    transport.delete_durable_object(&authority).await.unwrap();
    repository
        .finish_object_delete(counter, object_id, fenced.generation, 51)
        .unwrap();
    let recreated = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/increment?name=alpha",
    )
    .await;
    assert_eq!((recreated.status, recreated.body.as_str()), (200, "A:1"));
    let alpha_generations = repository
        .list_objects(account, counter)
        .unwrap()
        .into_iter()
        .filter(|object| object.object_id == object_id)
        .map(|object| object.generation)
        .collect::<Vec<_>>();
    assert_eq!(alpha_generations, vec![1, 2]);

    let expected = workers
        .list_versions(account, worker.id)
        .unwrap()
        .into_iter()
        .filter(|version| version.deleted_at_ms.is_none())
        .map(|version| version.id)
        .collect::<Vec<_>>();
    workers
        .delete_worker(account, worker.id, &expected, RequestId::generate(), 60)
        .unwrap();
    let fenced_after_worker_delete = repository
        .begin_object_delete(account, counter, object_id, 61)
        .unwrap();
    let purge_authority = repository
        .deletion_authority(
            account,
            counter,
            object_id,
            fenced_after_worker_delete.generation,
        )
        .unwrap();
    transport
        .delete_durable_object(&purge_authority)
        .await
        .unwrap();
    repository
        .finish_object_delete(
            counter,
            object_id,
            fenced_after_worker_delete.generation,
            62,
        )
        .unwrap();

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.7 identity/fetch/RPC/SQL/parallel/promotion/rollback/restart/delete/purge PASS");
}

fn durable_objects_config() -> DurableObjectsConfig {
    DurableObjectsConfig {
        disk_high_watermark_percent: 98,
        disk_stop_writes_percent: 99,
        ..DurableObjectsConfig::default()
    }
}

fn create_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account_id: AccountId,
    worker_id: WorkerId,
    class_name: &str,
    key: &str,
    now_ms: i64,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(storage, worker_id, class_name);
    match ResourceController::new(storage, pins, driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: format!("{key}-namespace"),
            idempotency_key: format!("p0-7-{key}"),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
    supervisor: &WorkerdSupervisor,
) -> VersionRecord {
    match controller
        .create_version(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "version failed: {error:?}; runtime={:?}; diagnostics={:?}",
                supervisor.snapshot(),
                supervisor.last_diagnostics()
            )
        }) {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

#[allow(clippy::too_many_arguments)]
fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    counter: ResourceId,
    other: ResourceId,
    output_queue: ResourceId,
    key: &str,
    release: &str,
    now_ms: i64,
    promote: bool,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: include_str!("../../../test/runtime/fixtures/durable-objects/counter.js")
                .as_bytes()
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    for (name, id) in [("COUNTER", counter), ("OTHER", other)] {
        bindings.insert(
            name.to_owned(),
            VersionBindingInput {
                kind: BindingKind::DoNamespace,
                id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
    }
    bindings.insert(
        "EVENTS".to_owned(),
        VersionBindingInput {
            kind: BindingKind::QueueProducer,
            id: output_queue,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    let mut vars = BTreeMap::new();
    vars.insert("RELEASE".to_owned(), serde_json::json!(release));
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::ScriptUpload),
        request_id: RequestId::generate(),
        now_ms,
    }
}

#[derive(Debug)]
struct DispatchResponse {
    status: u16,
    body: String,
}

fn assert_rpc_capability(response: &DispatchResponse, release: &str) {
    assert_eq!(response.status, 200, "{}", response.body);
    let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(value["direct"], format!("{release}:ok"));
    assert_eq!(value["property"], format!("{release}:capability"));
    assert_eq!(value["nested"], format!("{release}:nested:ok"));
    assert_eq!(value["envelope"], format!("{release}:ok"));
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "do.test")
        .body(Body::empty())
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: None,
                route_generation: i64::try_from(route_generation).unwrap(),
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap_or_else(|error| panic!("dispatch {path} failed: {error:?}"));
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, old_pid: i32, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000
"#,
        mock.endpoint
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 64 * 1024 * 1024).unwrap())
}

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::local_test_config().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    // This cohesive recovery matrix intentionally kills one runtime generation
    // for each independent crash boundary it verifies.
    config.restart_budget = 12;
    config
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
