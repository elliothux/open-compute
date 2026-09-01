//! Default Node surface on the pinned date, without tenant flags.

use super::*;
use futures::FutureExt as _;
use std::panic::AssertUnwindSafe;

const NODE_WORKER: &str = include_str!("../fixtures/nodejs_default_surface.js");
const PLATFORM_SECRET: &str = "platform-node-isolation-secret";
const PLATFORM_SECRET_DIGEST: &str =
    "b8b599f25c77094a7cbe8f747e0590d417893c011258a2cc543d617f5da0783f";
const TENANT_SECRET: &str = "tenant-node-token";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_2_nodejs_default_surface_isolation_and_unsupported_stubs() {
    assert!(
        std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").is_some(),
        "OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime"
    );

    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let host_probe = temp.path().join("host-secret.txt");
    std::fs::write(&host_probe, PLATFORM_SECRET).unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let mut binding_shutdown_rx = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(storage, Arc::new(SystemClock))),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                async move {
                    let _ = binding_shutdown_rx.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.2-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(auth.clone(), supervisor_slot.clone());
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-2-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();

    let outcome = AssertUnwindSafe(exercise(
        &supervisor,
        &transport,
        &storage,
        artifacts,
        host_probe.to_string_lossy().into_owned(),
    ))
    .catch_unwind()
    .await;
    supervisor.shutdown().await;
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert!(supervisor.snapshot().pid.is_none());
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

async fn exercise(
    supervisor: &WorkerdSupervisor,
    transport: &WorkerdTransport,
    storage: &Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    host_probe: String,
) {
    wait_running(supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "nodejs-gate", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller =
        DeploymentController::new(storage, artifacts, validator, BundleLimits::default());

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: NODE_WORKER.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("GREETING".to_owned(), serde_json::json!("from-var"));
    vars.insert("HOST_PROBE_PATH".to_owned(), serde_json::json!(host_probe));
    vars.insert(
        "HOST_PROBE_DIGEST".to_owned(),
        serde_json::json!(PLATFORM_SECRET_DIGEST),
    );
    let mut secrets = BTreeMap::new();
    secrets.insert("TOKEN".to_owned(), SecretString::new(TENANT_SECRET));
    let request = CreateDeploymentRequest {
        account_id: account,
        worker_id: worker.id,
        idempotency_key: "deploy-nodejs-default".to_owned(),
        content: open_compute_workers::DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets,
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: false,
        request_id: RequestId::generate(),
        now_ms: 21,
    };
    let deployment = match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    };
    let response = dispatch(transport, account, worker.id, &deployment, None, "").await;
    assert_eq!(
        response.status, 200,
        "unexpected Node dispatch response: {response:?}"
    );
    let body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["buffer"], "node-compat");
    assert_eq!(
        body["digest"],
        "6b5480dd23759dd0d3565b7593244eef13cf0129990307f8c8fee9c924ae54d8"
    );
    assert_eq!(body["path"], "a/b");
    assert_eq!(body["globalBuffer"], true);
    assert_eq!(body["greeting"], "from-var");
    assert_eq!(body["hasToken"], true);
    assert_eq!(
        body["envKeys"],
        serde_json::json!(["GREETING", "HOST_PROBE_DIGEST", "HOST_PROBE_PATH", "TOKEN"])
    );
    assert_eq!(body["processGreeting"], "from-var");
    assert_eq!(body["processToken"], TENANT_SECRET);
    assert_eq!(body["processPlatformSecret"], serde_json::Value::Null);
    assert_eq!(body["processPath"], serde_json::Value::Null);
    assert_eq!(body["processHome"], serde_json::Value::Null);
    let process_keys = body["processEnvKeys"]
        .as_array()
        .expect("process.env keys")
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(process_keys.contains(&"GREETING"));
    assert!(process_keys.contains(&"TOKEN"));
    assert!(process_keys.contains(&"HOST_PROBE_PATH"));
    assert!(
        !process_keys
            .iter()
            .any(|key| key.starts_with("OPEN_COMPUTE_"))
    );
    assert!(!process_keys.contains(&"PATH"));
    assert!(!process_keys.contains(&"HOME"));
    assert_eq!(
        body["hostFs"]["digestMatched"], false,
        "node:fs must not read host sentinel content"
    );
    assert_eq!(body["childProcess"]["threw"], true);
    assert_eq!(body["childProcess"]["code"], "ERR_METHOD_NOT_IMPLEMENTED");
    assert_eq!(body["sockets"]["imported"], true);
    assert_eq!(
        body["sockets"]["hasConnect"], true,
        "stable cloudflare:sockets must export connect"
    );

    let negative = dispatch(
        transport,
        account,
        worker.id,
        &deployment,
        None,
        "raw-tcp-negative",
    )
    .await;
    assert_eq!(negative.status, 200, "raw TCP negative probe: {negative:?}");
    let negative: serde_json::Value = serde_json::from_str(&negative.body).unwrap();
    for result in negative["sockets"].as_array().unwrap() {
        assert_eq!(result["opened"], false);
        assert_eq!(
            result["denied"], true,
            "public Network did not classify a private peer: {result}"
        );
    }
    for result in negative["malformed"].as_array().unwrap() {
        assert_eq!(result["opened"], false);
    }
    assert_eq!(negative["invalidTransport"]["opened"], false);
    for result in negative["node"].as_array().unwrap() {
        assert_eq!(result["opened"], false);
        assert_eq!(
            result["denied"], true,
            "node:net bypassed the public Network: {result}"
        );
        assert_eq!(result["timeout"], false);
    }
}
