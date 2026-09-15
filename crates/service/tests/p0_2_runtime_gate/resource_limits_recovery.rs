//! W2 `#67` acceptance: invocation resource limits protect neighbors, and a wedged runtime
//! generation recovers without restarting `ocd`.
//!
//! Both defenses run against the real pinned workerd, real SQLite, and the real system
//! gateway: (1) a busy-looping tenant is CPU-terminated with a stable limits outcome, its
//! isolate is evicted and rebuilt from the immutable Version, and neighbors plus the workerd
//! process are untouched; (2) a generic runtime stall (process frozen with the port and
//! control channel still present) is confirmed by the functional watchdog and recovered with
//! one generation restart.

use super::*;
use futures::FutureExt as _;
use std::panic::AssertUnwindSafe;

const TENANT_A: &str = r#"
export default {
  async fetch(request) {
    const mode = await request.text();
    if (mode === "spin") {
      let x = 0;
      while (true) { x = (x + 1) | 0; }
    }
    return Response.json({ tenant: "a" });
  }
};
"#;

const TENANT_B: &str = r#"
export default {
  async fetch() {
    return Response.json({ tenant: "b" });
  }
};
"#;

const TENANT_C: &str = r#"
export default {
  async fetch() {
    const results = [];
    for (let i = 0; i < 3; i++) {
      try {
        await fetch("https://example.invalid/");
        results.push("fetched");
      } catch (error) {
        results.push(
          String(error && error.message ? error.message : error).includes("Too many subrequests")
            ? "budget"
            : "outbound",
        );
      }
    }
    return Response.json({ results });
  }
};
"#;

const TENANT_D: &str = r#"
export default {
  async fetch() {
    for (let i = 0; i < 2; i++) {
      try {
        await fetch("https://example.invalid/");
      } catch {}
    }
    await fetch("https://example.invalid/");
    return new Response("unreachable");
  }
};
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w2_resource_limits_protect_neighbors_and_recover_a_wedged_generation() {
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
            version: "w2-limits-gate".to_owned(),
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
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("w2-limits-gate.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![auth.clone(), binding_auth.clone()],
    ));
    supervisor.set_watchdog_for_test(open_compute_runtime::WatchdogConfig {
        probe_interval: Duration::from_millis(250),
        probe_timeout: Duration::from_millis(500),
        failure_threshold: 3,
    });
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();

    let outcome = AssertUnwindSafe(exercise(&supervisor, &transport, &storage, artifacts))
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
) {
    wait_running(supervisor, Duration::from_secs(30)).await;
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(storage, artifacts, validator, BundleLimits::default());

    let (worker_a, version_a) = deploy(
        &controller,
        &repo,
        account,
        "w2-limits-a",
        TENANT_A,
        Some(VersionResourceLimitsInput {
            cpu_ms: Some(100),
            sub_requests: None,
        }),
    )
    .await;
    let (worker_b, version_b) =
        deploy(&controller, &repo, account, "w2-limits-b", TENANT_B, None).await;
    let (worker_c, version_c) = deploy(
        &controller,
        &repo,
        account,
        "w2-limits-c",
        TENANT_C,
        Some(VersionResourceLimitsInput {
            cpu_ms: None,
            sub_requests: Some(2),
        }),
    )
    .await;
    let (worker_d, version_d) = deploy(
        &controller,
        &repo,
        account,
        "w2-limits-d",
        TENANT_D,
        Some(VersionResourceLimitsInput {
            cpu_ms: None,
            sub_requests: Some(2),
        }),
    )
    .await;
    assert_startup_limit_rejected(&controller, &repo, account).await;

    // Defense 1: a busy-looping tenant is CPU-terminated with a stable limits outcome while
    // the runtime generation, neighbors, and persisted versions stay intact.
    let baseline = supervisor.snapshot();
    let baseline_pid = baseline.pid.expect("running pid");
    let baseline_startup = baseline.startup_id.expect("running startup id");

    let spin_started = Instant::now();
    let exceeded = dispatch(transport, account, worker_a.id, &version_a, None, "spin").await;
    let spin_elapsed = spin_started.elapsed();
    assert_eq!(
        exceeded.status, 500,
        "busy loop must surface a stable limits outcome: {exceeded:?}"
    );
    assert!(
        exceeded.body.contains("RESOURCE_LIMIT_EXCEEDED"),
        "limits outcome must carry the stable code: {exceeded:?}"
    );
    let exceeded_body: serde_json::Value = serde_json::from_str(&exceeded.body).unwrap();
    assert_eq!(exceeded_body["error"]["cloudflareCode"], 1102);
    assert_eq!(exceeded_body["error"]["outcome"], "exceededCpu");
    assert_eq!(exceeded.cf_error_type.as_deref(), Some("1102"));
    assert!(
        spin_elapsed < Duration::from_secs(20),
        "CPU termination must not wait for the bridge header timeout: {spin_elapsed:?}"
    );

    // The condemned isolate never serves again through retained stubs, but the immutable
    // Version still admits fresh invocations from a rebuilt isolate.
    let recovered = dispatch(transport, account, worker_a.id, &version_a, None, "").await;
    assert_eq!(recovered.status, 200, "tenant A rebuild: {recovered:?}");

    // Neighbors and the runtime generation are untouched by the tenant limit.
    let neighbor = dispatch(transport, account, worker_b.id, &version_b, None, "").await;
    assert_eq!(neighbor.status, 200, "neighbor tenant B: {neighbor:?}");
    let after = supervisor.snapshot();
    assert_eq!(after.pid, Some(baseline_pid));
    assert_eq!(after.startup_id, Some(baseline_startup));

    // The declared subrequest budget allows exactly two outbound attempts.
    let budget = dispatch(transport, account, worker_c.id, &version_c, None, "").await;
    assert_eq!(budget.status, 200, "subrequest budget probe: {budget:?}");
    let body: serde_json::Value = serde_json::from_str(&budget.body).unwrap();
    assert_eq!(
        body["results"],
        serde_json::json!(["outbound", "outbound", "budget"]),
        "the third subrequest must fail closed before any side effect"
    );
    let uncaught_budget = dispatch(transport, account, worker_d.id, &version_d, None, "").await;
    assert_eq!(uncaught_budget.status, 500, "{uncaught_budget:?}");
    let body: serde_json::Value = serde_json::from_str(&uncaught_budget.body).unwrap();
    assert_eq!(
        body["error"]["code"], "RESOURCE_LIMIT_EXCEEDED",
        "uncaught subrequest limit response: {body}"
    );
    assert_eq!(body["error"]["cloudflareCode"], 1101);
    assert_eq!(body["error"]["outcome"], "exception");
    assert_eq!(uncaught_budget.cf_error_type.as_deref(), Some("1101"));

    // Defense 2: a generic runtime stall (process frozen; port and control channel present)
    // is confirmed by the functional watchdog and recovered with one generation restart,
    // without restarting the platform.
    let stalled_pid = baseline_pid;
    let stopped = std::process::Command::new("kill")
        .args(["-STOP", &stalled_pid.to_string()])
        .status()
        .expect("SIGSTOP the runtime child");
    assert!(stopped.success(), "SIGSTOP must be applied to the child");

    // The supervisor must confirm and restart the generation on its own.
    wait_pid_change(supervisor, stalled_pid, Duration::from_secs(90)).await;
    let generation = supervisor.snapshot();
    let new_pid = generation.pid.expect("restarted pid");
    assert_ne!(new_pid, stalled_pid);
    assert_ne!(
        generation.startup_id.expect("restarted startup id"),
        baseline_startup,
        "a confirmed fault must rotate the generation identity"
    );
    // The frozen child is eventually reaped, leaving no orphan.
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::process::Command::new("kill")
        .args(["-0", &stalled_pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
    {
        assert!(Instant::now() < deadline, "stalled child was not reaped");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // After the new generation is ready, neighbors and the immutable versions serve again.
    let after_restart = dispatch(transport, account, worker_b.id, &version_b, None, "").await;
    assert_eq!(
        after_restart.status, 200,
        "tenant B after restart: {after_restart:?}"
    );
    let body: serde_json::Value = serde_json::from_str(&after_restart.body).unwrap();
    assert_eq!(body["tenant"], "b");
    let a_again = dispatch(transport, account, worker_a.id, &version_a, None, "").await;
    assert_eq!(a_again.status, 200, "tenant A after restart: {a_again:?}");
    let versions = repo
        .list_versions(account, worker_a.id)
        .unwrap()
        .into_iter()
        .filter(|version| version.state == VersionState::Ready)
        .count();
    assert_eq!(versions, 1, "immutable Version authority is preserved");
}

async fn assert_startup_limit_rejected(
    controller: &VersionController<'_>,
    repo: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
) {
    let (worker, _) = repo
        .create_worker(
            account,
            "w2-startup-limit",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"while (true) {}\nexport default { fetch() { return new Response('no'); } };"
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let error = controller
        .create_version(CreateVersionRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: "deploy-w2-startup-limit".to_owned(),
            content: open_compute_workers::VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            runtime_features: VersionRuntimeFeatures::default(),
            queue_consumers: Vec::new(),
            crons: Vec::new(),
            deployment_source: None,
            request_id: RequestId::generate(),
            now_ms: 42,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BundleRuntimeInvalid);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        None
    );
}

async fn deploy(
    controller: &VersionController<'_>,
    repo: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    name: &str,
    source: &str,
    limits: Option<VersionResourceLimitsInput>,
) -> (
    open_compute_storage::WorkerRecord,
    open_compute_storage::VersionRecord,
) {
    let (worker, _) = repo
        .create_worker(account, name, RequestId::generate(), 1, 1_000_000)
        .unwrap();
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
    let request = CreateVersionRequest {
        account_id: account,
        worker_id: worker.id,
        idempotency_key: format!("deploy-{name}"),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: VersionRuntimeFeatures {
            limits,
            ..Default::default()
        },
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: None,
        request_id: RequestId::generate(),
        now_ms: 42,
    };
    let version = match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    };
    (worker, version)
}
