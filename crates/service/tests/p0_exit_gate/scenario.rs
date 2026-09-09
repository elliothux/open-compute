use super::*;

pub(super) async fn p0_real_combined_exit_matrix_inner() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    reset_capacity_samples();
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let data_root = temp.path().join("data");
    let config = storage_config(&data_root);
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let scheduler_store = open_scheduler(&storage);
    let mock = MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    storage
        .bind_object_authority(ObjectStorageKind::S3, &objects.authority_sha256())
        .unwrap();
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd.clone(),
        lock.clone(),
        assets.clone(),
        "p0-exit-owner",
    )
    .await;

    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "p0-combined", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let router = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects.clone(),
        &pins,
        &stack,
    );
    let (bindings, do_plan) =
        create_product_set(&storage, &objects, &pins, account, worker.id).await;
    let public_ids = v4_product_ids(&router).await;
    apply_primary_d1_migration(&stack, account, bindings.d1).await;

    let version_a = {
        let validator: Arc<dyn RuntimeValidator> = Arc::new(stack.transport.clone());
        let controller = VersionController::new(
            &storage,
            artifacts.clone(),
            validator,
            BundleLimits::default(),
        )
        .with_durable_object_migration(do_plan);
        deploy(
            &controller,
            version_request(
                account,
                worker.id,
                bindings,
                "p0-exit-deploy-a",
                "A",
                true,
                20,
            ),
            &stack.supervisor,
        )
        .await
    };
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;

    let seeded = dispatch(
        &stack.transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/seed",
    )
    .await;
    let seed = response_json(&seeded);
    assert_snapshot(&seed, "A", "seed-kv", "seed-d1");
    assert_eq!(seed["durableObject"]["rpc"]["count"], 1);
    assert_eq!(seed["durableObject"]["isolated"]["count"], 1);

    let websocket = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/websocket",
        )
        .await,
    );
    assert_eq!(websocket, json!({"text": true, "binary": true}));

    let saturation = futures::future::join_all((0..16).map(|_| {
        dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/snapshot",
        )
    }))
    .await;
    for response in saturation {
        assert_snapshot(&response_json(&response), "A", "seed-kv", "seed-d1");
    }

    let kv_backup = create_backup(
        &router,
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/namespaces/{}/backups",
            public_ids.account, public_ids.kv
        ),
        "p0-exit-kv-backup",
    )
    .await;
    let d1_backup = create_backup(
        &router,
        &format!(
            "/client/v4/accounts/{}/open-compute/d1/databases/{}/backups",
            public_ids.account, public_ids.d1
        ),
        "p0-exit-d1-backup",
    )
    .await;
    assert!(
        mock.keys()
            .iter()
            .any(|key| key.contains("system/backups/kv/"))
    );
    assert!(
        mock.keys()
            .iter()
            .any(|key| key.contains("system/backups/d1/"))
    );

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/mutate",
        )
        .await,
    );
    let restored_kv = restore_resource(
        &router,
        &storage,
        account,
        BindingKind::KvNamespace,
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/backups/{kv_backup}/restore",
            public_ids.account
        ),
        "restored-kv",
        "p0-exit-kv-restore",
    )
    .await;
    let restored_d1 = restore_resource(
        &router,
        &storage,
        account,
        BindingKind::D1Database,
        &format!(
            "/client/v4/accounts/{}/open-compute/d1/backups/{d1_backup}/restore",
            public_ids.account
        ),
        "restored-d1",
        "p0-exit-d1-restore",
    )
    .await;
    assert_ne!(restored_kv, bindings.kv);
    assert_ne!(restored_d1, bindings.d1);

    let due = now_ms().saturating_sub(1).max(1);
    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
    let bindings_b = ProductBindings {
        kv: restored_kv,
        d1: restored_d1,
        ..bindings
    };
    let version_b = {
        let validator: Arc<dyn RuntimeValidator> = Arc::new(stack.transport.clone());
        let controller = VersionController::new(
            &storage,
            artifacts.clone(),
            validator,
            BundleLimits::default(),
        );
        deploy(
            &controller,
            version_request(
                account,
                worker.id,
                bindings_b,
                "p0-exit-deploy-b",
                "B",
                true,
                30,
            ),
            &stack.supervisor,
        )
        .await
    };
    let generation_b = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let alarm_b = alarm_status(&stack, account, worker.id, &version_b, generation_b).await;
    assert_eq!(alarm_b["alarmDeliveries"], 1);
    assert_eq!(alarm_b["alarmRelease"], "B");
    assert_eq!(alarm_b["alarmRetryCount"], 0);
    assert_eq!(alarm_b["alarm"], Value::Null);

    let restored = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_b,
            generation_b,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&restored, "B", "seed-kv", "seed-d1");

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_b,
            generation_b,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
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
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let alarm_a = alarm_status(&stack, account, worker.id, &version_a, generation_rollback).await;
    assert_eq!(alarm_a["alarmDeliveries"], 2);
    assert_eq!(alarm_a["alarmRelease"], "A");
    let rolled_back = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&rolled_back, "A", "mutated-kv", "mutated-d1");

    let killed_pid = stack.supervisor.snapshot().pid.unwrap();
    kill_workerd(killed_pid);
    wait_pid_change(&stack.supervisor, killed_pid, Duration::from_secs(30)).await;
    let after_workerd_crash = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&after_workerd_crash, "A", "mutated-kv", "mutated-d1");

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
    for id in all_resources(bindings_b) {
        assert_eq!(pins.count(id), 0);
    }
    drop(router);
    stack.stop().await;
    drop(scheduler_store);
    drop(storage);

    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let scheduler_store = open_scheduler(&storage);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd.clone(),
        lock.clone(),
        assets.clone(),
        "p0-exit-owner",
    )
    .await;
    let router = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects.clone(),
        &pins,
        &stack,
    );
    let persisted_worker = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap();
    assert_eq!(persisted_worker.active_version_id, Some(version_a.id));
    assert_eq!(persisted_worker.route_generation, generation_rollback);
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let after_platform_restart =
        alarm_status(&stack, account, worker.id, &version_a, generation_rollback).await;
    assert_eq!(after_platform_restart["alarmDeliveries"], 3);
    assert_eq!(after_platform_restart["alarmRelease"], "A");
    let persisted = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&persisted, "A", "mutated-kv", "mutated-d1");

    drop(router);
    stack.stop().await;
    drop(storage);
    let recovery_key = temp.path().join("p1-recovery-master.key");
    fs::copy(data_root.join("keys/master.key"), &recovery_key).unwrap();
    fs::set_permissions(&recovery_key, fs::Permissions::from_mode(0o600)).unwrap();
    let source_platform_config = write_platform_config(&PlatformConfigInput {
        temp: &temp,
        name: "p1-source",
        path: &data_root,
        master_key: &recovery_key,
        mock: &mock,
    });
    let source_loaded = load_file_only_platform_config(&source_platform_config);
    let full_snapshot = backup_create(&source_loaded, "p0-combined-fixture")
        .await
        .unwrap();
    assert!(
        backup_inspect(&source_loaded, &full_snapshot.snapshot_id, true)
            .await
            .unwrap()
            .verified
    );
    let retired_source = temp.path().join("p1-source-unavailable");
    fs::rename(&data_root, &retired_source).unwrap();

    let restored_root = fs::canonicalize(temp.path())
        .unwrap()
        .join("p1-restored-data");
    let restore_platform_config = write_platform_config(&PlatformConfigInput {
        temp: &temp,
        name: "p1-restore",
        path: &restored_root,
        master_key: &recovery_key,
        mock: &mock,
    });
    let restored_loaded = load_file_only_platform_config(&restore_platform_config);
    let restored = backup_restore(&restored_loaded, &full_snapshot.snapshot_id)
        .await
        .unwrap();
    assert_eq!(restored.platform_id, full_snapshot.platform_id);
    let doctor = doctor_report(&restored_loaded, DoctorMode::Full).await;
    assert!(!doctor.failed(), "restored doctor: {doctor:?}");

    let storage =
        Arc::new(PlatformStorage::bootstrap(&restored_loaded.config.data, &SystemClock).unwrap());
    let scheduler_store = open_scheduler(&storage);
    let (artifacts, objects) = stores(&mock);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd,
        lock,
        assets,
        "p0-exit-owner",
    )
    .await;
    let router = admin_router(storage.clone(), artifacts, objects, &pins, &stack);
    let restored_worker = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap();
    assert_eq!(restored_worker.active_version_id, Some(version_a.id));
    assert_eq!(restored_worker.route_generation, generation_rollback);
    let restored_snapshot = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&restored_snapshot, "A", "mutated-kv", "mutated-d1");

    let s3_request = tokio::spawn({
        let transport = stack.transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                generation_rollback,
                "/s3-fault?delay=250",
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    mock.set_fault(Fault::ServerError);
    let s3_failure = response_json(&s3_request.await.unwrap());
    assert!(
        s3_failure["r2Error"]
            .as_str()
            .unwrap()
            .contains("R2_PROVIDER_UNAVAILABLE")
    );
    assert_eq!(s3_failure["kv"], "mutated-kv");
    assert_eq!(s3_failure["d1"], "mutated-d1");
    let (failed_backup_status, failed_backup) = admin_json(
        &router,
        "POST",
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/namespaces/{}/backups",
            public_ids.account, public_ids.kv_other
        ),
        Value::Null,
        Some("p0-exit-s3-failed-backup"),
    )
    .await;
    assert_v4_envelope(failed_backup_status, &failed_backup);
    assert_eq!(failed_backup_status, StatusCode::SERVICE_UNAVAILABLE);
    let failure_text = failed_backup.to_string();
    assert!(!failure_text.contains(&mock.endpoint));
    assert!(!failure_text.contains("sqlite"));
    mock.set_fault(Fault::None);
    let after_s3_recovery = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&after_s3_recovery, "A", "mutated-kv", "mutated-d1");

    corrupt_d1(&storage, account, bindings.d1_corrupt);
    let isolated_corruption = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/corruption",
        )
        .await,
    );
    assert_eq!(isolated_corruption["primary"], "mutated-d1");
    assert_eq!(isolated_corruption["kv"], "mutated-kv");
    assert!(
        isolated_corruption["corruptError"]
            .as_str()
            .unwrap()
            .contains("D1_DATABASE_CORRUPT")
    );
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, bindings.d1_corrupt)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );

    let deleted = dispatch(
        &stack.transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/delete-r2",
    )
    .await;
    assert_eq!((deleted.status, deleted.body.as_str()), (200, "null"));
    for id in all_resources(bindings_b) {
        assert_eq!(pins.count(id), 0);
    }
    drop(router);
    stack.stop().await;
    drop(storage);
    let attestation =
        backup_attest_restore_smoke(&restored_loaded, &full_snapshot.snapshot_id, true)
            .await
            .unwrap();
    assert!(attestation.smoke_verified);
    println!("P1_CAPACITY {}", capacity_summary());
    println!("P0 combined Worker/KV/R2/D1/DO/alarm/WebSocket/backup/restart/failure matrix PASS");
}
