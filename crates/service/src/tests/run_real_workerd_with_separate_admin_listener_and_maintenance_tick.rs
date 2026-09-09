use super::*;

#[tokio::test]
async fn run_real_workerd_with_separate_admin_listener_and_maintenance_tick() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let mut loaded = load_fixture_platform_config(&path);
    loaded.config.runtime.startup_timeout_ms = 60_000;
    loaded.config.runtime.shutdown_grace_ms = 1_000;
    loaded.config.runtime.kill_timeout_ms = 2_000;
    loaded.config.workers.artifact_gc_interval_ms = 20;
    loaded.config.workers.version_min_retention_ms = 0;
    loaded.config.workers.retain_rejected_versions = 1;

    {
        let storage = open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap();
        let repo = open_compute_storage::WorkerRepository::new(storage.db());
        let account = storage.identity().default_account_id;
        let (worker, _) = repo
            .create_worker(
                account,
                "maintenance-worker",
                open_compute_core::RequestId::generate(),
                1,
                1_000_000,
            )
            .unwrap();
        for (index, timestamp) in [(1_u8, 2_i64), (2, 3)] {
            let version = open_compute_core::VersionId::generate();
            repo.insert_staging_version(
                &open_compute_storage::NewVersion {
                    id: version,
                    account_id: account,
                    worker_id: worker.id,
                    content_kind: open_compute_storage::VersionContentKind::Worker,
                    artifact_sha256: Some([index; 32]),
                    artifact_size: Some(u64::from(index)),
                    artifact_schema_version: Some(1),
                    main_module: Some("index.js".to_owned()),
                    worker_code_sha256: [index.saturating_add(10); 32],
                    compatibility_date: "2026-09-08".into(),
                    compatibility_flags: Vec::new(),
                    vars: std::collections::BTreeMap::new(),
                    secrets: std::collections::BTreeMap::new(),
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: timestamp,
                },
                &open_compute_storage::NewVersionProducts::default(),
                1_000_000,
            )
            .unwrap();
            repo.mark_rejected(
                version,
                open_compute_storage::VersionState::Staging,
                ErrorCode::BundleInvalid,
                timestamp,
            )
            .unwrap();
        }
    }

    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let admin_addr = reserved.local_addr().unwrap();
    drop(reserved);
    loaded.config.server.admin_bind = Some(admin_addr.to_string());

    let options = RunOptions::default();
    let addresses = options.last_public_addr.clone();
    let mut task = tokio::spawn(run_platform_with(loaded, options));
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if addresses.lock().unwrap().is_some() {
                break;
            }
            if task.is_finished() {
                panic!("platform startup ended early: {:?}", (&mut task).await);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(mock.object_count(), 1);
}
