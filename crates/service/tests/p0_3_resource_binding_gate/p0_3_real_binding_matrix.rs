use super::*;

pub(super) async fn run() {
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

    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let pins = ResourcePins::new();
    let fake = Arc::new(FakeState::default());
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
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        let executor = Arc::new(FakeExecutor(fake.clone()));
        async move {
            serve_binding_backend(
                binding_listener,
                storage,
                auth,
                pins,
                executor,
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
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
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.3-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone());
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-3-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let controller = ResourceController::new(&storage, pins.clone(), FakeDriver(fake.clone()));
    let resource = create_resource(&controller, account, "cache", "resource-create", 10);
    assert!(matches!(
        controller
            .create(&resource_request(account, "cache", "resource-create", 11))
            .unwrap(),
        CreateResourceOutcome::Replay(bytes) if !bytes.is_empty()
    ));
    let repository = WorkerRepository::new(storage.db());
    let (worker, _) = repository
        .create_worker(
            account,
            "binding-gate",
            RequestId::generate(),
            12,
            1_000_000,
        )
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );

    let collision = versions
        .create_version(version_request(
            account,
            worker.id,
            "env-collision",
            Some((resource, CanonicalPermissions::default())),
            true,
            true,
            20,
        ))
        .await
        .unwrap_err();
    assert_eq!(collision.code(), ErrorCode::BindingTypeMismatch);

    let foreign = AccountId::generate();
    insert_account(storage.data_dir().control_db_path(), foreign);
    let (foreign_worker, _) = repository
        .create_worker(foreign, "foreign", RequestId::generate(), 21, 1_000_000)
        .unwrap();
    let cross_account = versions
        .create_version(version_request(
            foreign,
            foreign_worker.id,
            "cross-account",
            Some((resource, CanonicalPermissions::default())),
            false,
            false,
            22,
        ))
        .await
        .unwrap_err();
    assert_eq!(cross_account.code(), ErrorCode::ResourceNotFound);

    let bound = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            "bound",
            Some((resource, CanonicalPermissions::default())),
            true,
            false,
            30,
        ),
    )
    .await;
    let put = dispatch(&transport, account, worker.id, &bound, "/put", "alpha").await;
    assert_eq!(put.status, 200, "{}", put.body);
    assert_eq!(put.loader_outcome, Some(LoaderOutcome::Cold));
    let get = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!((get.status, get.body.as_str()), (200, "alpha"));
    assert_eq!(get.loader_outcome, Some(LoaderOutcome::Warm));

    ResourceRepository::new(storage.db())
        .rename(
            account,
            resource,
            "renamed-cache",
            RequestId::generate(),
            31,
        )
        .unwrap();
    let renamed = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!(renamed.body, "alpha");
    let props = dispatch(&transport, account, worker.id, &bound, "/props", "").await;
    assert_eq!(props.status, 200);
    assert!(!props.body.contains(&resource.to_string()));
    assert!(!props.body.contains("BINDING_BACKEND"));
    let streamed = dispatch(
        &transport,
        account,
        worker.id,
        &bound,
        "/stream",
        "stream-ok",
    )
    .await;
    assert_eq!(
        (streamed.status, streamed.body.as_str()),
        (200, "stream-ok")
    );
    assert_eq!(pins.count(resource), 0);

    let binding = BindingRepository::new(storage.db())
        .version_bindings(bound.id)
        .unwrap()
        .pop()
        .unwrap();
    let current_token = binding_auth.credential().unwrap();
    let generation = binding_auth.claimed_generation_for_test().unwrap();
    let forged_token = backend_call(
        binding_addr,
        &"00".repeat(32),
        &generation,
        binding.id,
        bound.id,
        &hex::encode(binding.descriptor_sha256),
        "get",
        br#"{"keys":["k"]}"#,
        None,
    )
    .await;
    assert_eq!(forged_token.status(), StatusCode::NOT_FOUND);
    assert!(
        forged_token
            .headers()
            .get("x-open-compute-error-code")
            .is_none()
    );
    let forged_hash = backend_call(
        binding_addr,
        current_token.expose(),
        &generation,
        binding.id,
        bound.id,
        &"00".repeat(32),
        "get",
        br#"{"keys":["k"]}"#,
        None,
    )
    .await;
    assert_eq!(
        forged_hash
            .headers()
            .get("x-open-compute-error-code")
            .unwrap(),
        ErrorCode::BindingTypeMismatch.as_str()
    );
    let oversized = backend_call(
        binding_addr,
        current_token.expose(),
        &generation,
        binding.id,
        bound.id,
        &hex::encode(binding.descriptor_sha256),
        "put",
        b"",
        Some(open_compute_storage::KV_MAX_VALUE_BYTES + 64 * 1024 + 1),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let original_hash = binding.descriptor_sha256;
    tamper_descriptor(storage.data_dir().control_db_path(), binding.id, [0; 32]);
    let warm_tamper = dispatch(&transport, account, worker.id, &bound, "/get", "").await;
    assert_eq!(warm_tamper.status, 500);
    assert!(warm_tamper.body.contains("VERSION_INVARIANT_VIOLATION"));
    assert_eq!(
        repository
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(bound.id)
    );
    tamper_descriptor(
        storage.data_dir().control_db_path(),
        binding.id,
        original_hash,
    );

    let read_only = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            "read-only",
            Some((
                resource,
                CanonicalPermissions {
                    read: true,
                    write: false,
                },
            )),
            true,
            false,
            40,
        ),
    )
    .await;
    let denied = dispatch(&transport, account, worker.id, &read_only, "/put", "denied").await;
    assert_eq!(denied.status, 500);
    assert!(denied.body.contains("BINDING_PERMISSION_DENIED"));

    fake.values
        .lock()
        .unwrap()
        .get_mut(&resource)
        .unwrap()
        .insert(
            "gate".to_owned(),
            vec![b'x'; open_compute_storage::KV_MAX_VALUE_BYTES + 1],
        );
    let result_limit = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(result_limit.status, 500);
    assert!(result_limit.body.contains("KV_VALUE_TOO_LARGE"));
    fake.values
        .lock()
        .unwrap()
        .get_mut(&resource)
        .unwrap()
        .insert("gate".to_owned(), b"alpha".to_vec());

    ResourceRepository::new(storage.db())
        .set_availability(
            account,
            resource,
            ResourceAvailability::Unavailable,
            Some("FAKE_UNAVAILABLE"),
            41,
        )
        .unwrap();
    let isolated = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(isolated.status, 500);
    assert!(isolated.body.contains("RESOURCE_UNAVAILABLE"));
    ResourceRepository::new(storage.db())
        .set_availability(account, resource, ResourceAvailability::Healthy, None, 42)
        .unwrap();

    let delete_referenced = controller
        .delete(
            account,
            resource,
            RequestId::generate(),
            43,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
    assert_eq!(delete_referenced.code(), ErrorCode::ResourceReferenced);

    let old_pid = supervisor.snapshot().pid.unwrap();
    let old_token = current_token.expose().to_owned();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let stale = backend_call(
        binding_addr,
        &old_token,
        &generation,
        binding.id,
        bound.id,
        &hex::encode(original_hash),
        "get",
        br#"{"keys":["gate"]}"#,
        None,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::NOT_FOUND);
    let post_restart = dispatch(&transport, account, worker.id, &read_only, "/get", "").await;
    assert_eq!(post_restart.body, "alpha");

    let held = pins.try_pin(resource).unwrap();
    let drain = pins.fence_and_wait(resource, Duration::from_secs(1));
    tokio::pin!(drain);
    assert!(
        tokio::time::timeout(Duration::from_millis(5), &mut drain)
            .await
            .is_err()
    );
    drop(held);
    drain.await.unwrap();
    pins.unfence(resource);

    let plain = deploy(
        &versions,
        version_request(account, worker.id, "plain", None, true, false, 50),
    )
    .await;
    let plain_result = dispatch(&transport, account, worker.id, &plain, "/plain", "").await;
    assert_eq!(
        (plain_result.status, plain_result.body.as_str()),
        (200, "plain")
    );
    repository
        .prune_expired_idempotency(24 * 60 * 60 * 1000 + 100, 100)
        .unwrap();
    delete_version(repository, account, worker.id, bound.id, 51);
    delete_version(repository, account, worker.id, read_only.id, 52);
    controller
        .delete(
            account,
            resource,
            RequestId::generate(),
            53,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(pins.count(resource), 0);
    let recreated = create_resource(&controller, account, "renamed-cache", "recreate", 54);
    assert_ne!(recreated, resource);

    let diagnostics = format!("{:?}", supervisor.last_diagnostics());
    assert!(!diagnostics.contains(&old_token));
    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert_eq!(pins.count(recreated), 0);
    println!("RB-01..RB-18 PASS");
}
