use super::*;

pub(super) async fn run() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let artifact_cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .unwrap();

    let source_listener =
        bind_runtime_source(&storage.data_dir().runtime_dir().join("source.sock"))
            .await
            .unwrap();

    let binding_listener =
        bind_binding_backend(&storage.data_dir().runtime_dir().join("binding.sock"))
            .await
            .unwrap();
    let source_addr = source_listener.path().to_owned();
    let binding_addr = binding_listener.path().to_owned();
    let version_pins = VersionPins::new();
    let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown.subscribe();
    let source_task = tokio::spawn({
        let source = RuntimeSource::new(
            storage.clone(),
            artifacts.clone(),
            BundleLimits::default(),
            open_compute_service::runtime_bridge::python_runtime_pin(&runtime),
        )
        .unwrap()
        .with_cache(artifact_cache.clone())
        .with_cache_fail_open(true);

        async move {
            serve_runtime_source(source_listener, source, None, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let cache_metrics = Arc::new(
        open_compute_service::metrics::MetricsRegistry::new(
            &open_compute_core::MetricsConfig::default(),
            "cache-gate",
            "workerd",
        )
        .unwrap(),
    );
    let cache_service = Arc::new(
        CacheBindingService::new(
            storage.clone(),
            artifacts.clone(),
            artifact_cache.clone(),
            ResponseCacheConfig::default(),
        )
        .unwrap()
        .with_metrics(cache_metrics.clone()),
    );
    let cache_manager = cache_service.manager();
    let image_service = Arc::new(ImageBindingService::new(
        storage.clone(),
        ImagesConfig::default(),
    ));
    let services = Arc::new(ServiceInvocationRegistry::new(
        storage.clone(),
        version_pins.clone(),
    ));
    let binding_task = tokio::spawn({
        let storage = storage.clone();

        let pins = version_pins.clone();
        let cache_service = cache_service.clone();
        let images = image_service.clone();
        let assets = Arc::new(AssetBindingService::new(
            storage.clone(),
            artifacts.clone(),
            artifact_cache.clone(),
            pins.clone(),
        ));
        let services = services.clone();
        async move {
            serve_binding_backend_with_assets(
                binding_listener,
                storage.clone(),
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(
                    storage.clone(),
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                assets,
                services,
                Some(cache_service),
                Some(images),
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock,
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        storage.data_dir().runtime_dir().join("ingress.sock"),
        PlatformReleaseMeta {
            version: "p3-cache-images-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    );
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport =
        WorkerdTransport::new(supervisor_slot.clone()).with_version_pins(version_pins.clone());
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().instance_id.to_string(),
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
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("p3-cache-images.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::unix("runtime-source", &source_addr).unwrap(),
            ExternalServiceAddress::unix("binding-backend", &binding_addr).unwrap(),
            ExternalServiceAddress::unix("observability-backend", &binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    // Standalone component Gates must wire the same generation cleanup as the daemon.
    let (generation_applied, mut generation_updates) = tokio::sync::watch::channel(None);
    let generation_task = tokio::spawn({
        let mut snapshots = supervisor.subscribe();
        let mut lifecycle_shutdown = shutdown.subscribe();
        let images = image_service.clone();
        let mut resources =
            open_compute_service::runtime_generation::RuntimeGenerationResources::new(
                services.as_ref().clone(),
                version_pins.clone(),
            );
        resources.observe(&snapshots.borrow().clone());
        async move {
            loop {
                let snapshot = snapshots.borrow_and_update().clone();
                if resources.observe(&snapshot).resources_cleared {
                    images.clear_sessions().unwrap();
                }
                generation_applied.send_replace(
                    snapshot
                        .startup_id
                        .filter(|_| snapshot.state == SupervisorState::Running),
                );
                tokio::select! {
                    changed = snapshots.changed() => { if changed.is_err() { break; } }
                    changed = lifecycle_shutdown.changed() => {
                        if changed.is_err() || *lifecycle_shutdown.borrow() { break; }
                    }
                }
            }
        }
    });

    let account = storage.identity().instance_id;
    let repo = WorkerRepository::new(storage.db());
    let target = repo
        .create_worker(account, "cache-target", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let caller = repo
        .create_worker(account, "cache-caller", RequestId::generate(), 2, 1_000_000)
        .unwrap()
        .0;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let pixel = pixel_base64();
    let a = deploy(
        &controller,
        request(
            account,
            target.id,
            "a",
            &target_source("A", &pixel),
            BTreeMap::new(),
            features("release-A"),
            true,
            10,
        ),
        &supervisor,
    )
    .await;

    let first = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(first.0, 200, "first automatic cache response: {first:?}");
    assert_eq!(first.1, "A:1");
    assert_eq!(first.2.as_deref(), Some("MISS"));
    assert!(first.3.is_none(), "Cache-Tag must not reach the client");
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        1,
        Duration::from_secs(5),
        &cache_metrics,
    )
    .await;
    let hit = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!((hit.1.as_str(), hit.2.as_deref()), ("A:1", Some("HIT")));
    let range_hit = dispatch_request(
        &transport,
        &repo,
        account,
        target.id,
        &a,
        Request::builder()
            .method(Method::GET)
            .uri("/auto")
            .header(header::HOST, "cache.example.test")
            .header(header::RANGE, "bytes=0-0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        (range_hit.0, range_hit.1.as_str(), range_hit.2.as_deref()),
        (206, "A", Some("HIT"))
    );

    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/api-put")
            .await
            .1,
        "put"
    );
    let range = dispatch(&transport, &repo, account, target.id, &a, "/api-range").await;
    assert_eq!((range.0, range.1.as_str()), (206, "tor"));
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            target.id,
            &a,
            "/api-encoding-put"
        )
        .await
        .1,
        "encoded"
    );
    for (path, expected_status) in [
        ("/api-encoding-match", None),
        ("/automatic-encoding", Some("MISS")),
        ("/automatic-encoding", Some("HIT")),
    ] {
        let before = cache_entries(&cache_manager, account, target.id);
        let response = dispatch(&transport, &repo, account, target.id, &a, path).await;
        assert_eq!(response.0, 200);
        let value: serde_json::Value = serde_json::from_str(&response.1).unwrap();
        if path == "/api-encoding-match" {
            assert_eq!(
                value,
                serde_json::json!({ "default": "gzip-A", "named": "stream-A" })
            );
        } else {
            assert_eq!(value["body"], "automatic-gzip-A");
            assert_eq!(value["status"].as_str(), expected_status);
            if expected_status == Some("MISS") {
                wait_cache_entries(
                    &cache_manager,
                    account,
                    target.id,
                    before + 1,
                    Duration::from_secs(5),
                    &cache_metrics,
                )
                .await;
            }
        }
    }
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            target.id,
            &a,
            "/api-conditional"
        )
        .await
        .0,
        304
    );
    let version: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/version")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(version["id"], a.id.to_string());
    assert_eq!(version["tag"], "release-A");
    assert_eq!(version["timestamp"], "1970-01-01T00:00:00.010Z");
    let image: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/images")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(
        (
            image["format"].as_str(),
            image["width"].as_u64(),
            image["height"].as_u64()
        ),
        (Some("png"), Some(2), Some(2))
    );
    assert_eq!(image["contentType"], "image/png");
    assert!(image["outputBytes"].as_u64().unwrap() > 0);

    let entries_before_ctx = cache_entries(&cache_manager, account, target.id);
    let ctx_first = dispatch(&transport, &repo, account, target.id, &a, "/ctx").await;
    assert_eq!(ctx_first.1, "A-named:1");
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_ctx + 1,
        Duration::from_secs(5),
        &cache_metrics,
    )
    .await;
    let ctx_hit = dispatch(&transport, &repo, account, target.id, &a, "/ctx").await;
    assert_eq!(ctx_hit.1, "A-named:1");

    let b = deploy(
        &controller,
        request(
            account,
            target.id,
            "b",
            &target_source("B", &pixel),
            BTreeMap::new(),
            features("release-B"),
            true,
            20,
        ),
        &supervisor,
    )
    .await;
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &b, "/api-match")
            .await
            .1,
        "stored-A"
    );
    let b_miss = dispatch(&transport, &repo, account, target.id, &b, "/auto").await;
    assert_eq!(
        (b_miss.1.as_str(), b_miss.2.as_deref()),
        ("B:1", Some("MISS"))
    );
    repo.promote(
        account,
        target.id,
        a.id,
        Some(b.id),
        RequestId::generate(),
        30,
    )
    .unwrap();
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/auto")
            .await
            .1,
        "A:1"
    );

    let shared_c = deploy(
        &controller,
        request(
            account,
            target.id,
            "shared-c",
            &target_source("C", &pixel),
            BTreeMap::new(),
            shared_features("release-C"),
            false,
            31,
        ),
        &supervisor,
    )
    .await;
    let entries_before_shared = cache_entries(&cache_manager, account, target.id);
    let shared_miss = dispatch(&transport, &repo, account, target.id, &shared_c, "/auto").await;
    assert_eq!(
        (shared_miss.1.as_str(), shared_miss.2.as_deref()),
        ("C:1", Some("MISS"))
    );
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_shared + 1,
        Duration::from_secs(5),
        &cache_metrics,
    )
    .await;
    let shared_d = deploy(
        &controller,
        request(
            account,
            target.id,
            "shared-d",
            &target_source("D", &pixel),
            BTreeMap::new(),
            shared_features("release-D"),
            false,
            32,
        ),
        &supervisor,
    )
    .await;
    let shared_hit = dispatch(&transport, &repo, account, target.id, &shared_d, "/auto").await;
    assert_eq!(
        (shared_hit.1.as_str(), shared_hit.2.as_deref()),
        ("C:1", Some("HIT"))
    );

    let services = BTreeMap::from([(
        "TARGET".to_owned(),
        VersionServiceInput {
            target: ServiceTarget::Worker {
                worker_id: target.id,
            },
            entrypoint: None,
            props: None,
        },
    )]);
    let caller_version = deploy(
        &controller,
        request(
            account,
            caller.id,
            "caller",
            CALLER_SOURCE,
            services,
            VersionRuntimeFeatures {
                compatibility_date: "2026-09-08".to_owned(),
                ..VersionRuntimeFeatures::default()
            },
            true,
            40,
        ),
        &supervisor,
    )
    .await;
    let entries_before_service = cache_entries(&cache_manager, account, target.id);
    let service_first = dispatch(
        &transport,
        &repo,
        account,
        caller.id,
        &caller_version,
        "/service",
    )
    .await;
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_service + 1,
        Duration::from_secs(5),
        &cache_metrics,
    )
    .await;
    let service_hit = dispatch(
        &transport,
        &repo,
        account,
        caller.id,
        &caller_version,
        "/service",
    )
    .await;
    assert!(service_first.1.starts_with("A:"));
    assert_eq!(service_hit.1, service_first.1);
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            caller.id,
            &caller_version,
            "/rpc"
        )
        .await
        .1,
        "1"
    );
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            caller.id,
            &caller_version,
            "/rpc"
        )
        .await
        .1,
        "2"
    );
    let purged: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/purge")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(purged["success"], true);
    assert!(purged["deleted"].as_u64().unwrap() >= 2);
    let entries_before_refill = cache_entries(&cache_manager, account, target.id);
    let after_purge = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(after_purge.2.as_deref(), Some("MISS"));
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_refill + 1,
        Duration::from_secs(5),
        &cache_metrics,
    )
    .await;
    let before_restart = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(
        (before_restart.1.as_str(), before_restart.2.as_deref()),
        (after_purge.1.as_str(), Some("HIT"))
    );
    let deleted: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/api-delete")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/api-match")
            .await
            .0,
        404
    );

    open_image_session(
        &image_service,
        &storage,
        account,
        target.id,
        &a,
        &base64::engine::general_purpose::STANDARD
            .decode(&pixel)
            .unwrap(),
    )
    .await;
    assert_eq!(image_service.capacity().unwrap().active_sessions, 1);

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.force_restart_for_test();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let restarted_generation = supervisor.snapshot().startup_id;
    tokio::time::timeout(
        Duration::from_secs(5),
        generation_updates.wait_for(|applied| *applied == restarted_generation),
    )
    .await
    .unwrap()
    .unwrap();
    let after_restart = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(
        (after_restart.1.as_str(), after_restart.2.as_deref()),
        (before_restart.1.as_str(), Some("HIT"))
    );
    let encoded = dispatch(
        &transport,
        &repo,
        account,
        target.id,
        &a,
        "/api-encoding-match",
    )
    .await;
    assert_eq!(encoded.0, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded.1).unwrap(),
        serde_json::json!({ "default": "gzip-A", "named": "stream-A" })
    );
    let automatic = dispatch(
        &transport,
        &repo,
        account,
        target.id,
        &a,
        "/automatic-encoding",
    )
    .await;
    assert_eq!(automatic.0, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&automatic.1).unwrap(),
        serde_json::json!({ "body": "automatic-gzip-A", "status": "HIT" })
    );
    let restarted_version: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/version")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(restarted_version, version);
    let restarted_image: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/images")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(restarted_image["format"], "png");
    assert_eq!(restarted_image["contentType"], "image/png");
    assert!(restarted_image["outputBytes"].as_u64().unwrap() > 0);
    assert_eq!(image_service.capacity().unwrap().active_sessions, 0);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    generation_task.await.unwrap();
}
