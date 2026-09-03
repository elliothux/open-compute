use super::*;
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, RequestId, SystemClock};
use open_compute_storage::{D1QueryLimits, D1Statement, D1Value};
use open_compute_storage::{D1TransferAction, D1TransferKind, D1TransferState, NewD1Transfer};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
};
use sha2::Sha256;
use std::sync::atomic::{AtomicBool, Ordering};

const QUOTA: u64 = 256 * 1024 * 1024;

fn fixture() -> (
    tempfile::TempDir,
    Arc<PlatformStorage>,
    AccountId,
    ResourceId,
    Arc<D1Coordinator>,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let resource = match ResourceController::new(
        storage.as_ref(),
        ResourcePins::new(),
        D1ResourceDriver::new(storage.as_ref(), QUOTA),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::D1Database,
        name: "coordinator-db".to_owned(),
        idempotency_key: "coordinator-db".to_owned(),
        driver_schema_version: open_compute_storage::D1_DATABASE_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms: 10,
    })
    .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    };
    let coordinator = Arc::new(D1Coordinator::new(
        storage.clone(),
        ResourcePins::new(),
        D1Config::default(),
    ));
    (temp, storage, account, resource, coordinator)
}

#[tokio::test]
async fn ordinary_mutations_do_not_depend_on_completed_history() {
    let (_temp, storage, account, resource, coordinator) = fixture();
    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.engine.user_version(),
        )
        .await
        .unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    assert_eq!(history.latest_snapshot(account, resource).unwrap(), None);

    coordinator
        .execute(account, resource, Duration::from_secs(2), true, |context| {
            context.mark_mutation();
            context.engine.exec(
                "CREATE TABLE history(value TEXT UNIQUE)",
                D1QueryLimits::query(context.config)?,
            )
        })
        .await
        .unwrap();
    assert_eq!(history.latest_snapshot(account, resource).unwrap(), None);

    let partial = coordinator
        .execute(account, resource, Duration::from_secs(2), true, |context| {
            context.mark_mutation();
            context.engine.exec(
                "INSERT INTO history VALUES ('prefix');
                 INSERT INTO history VALUES ('prefix')",
                D1QueryLimits::query(context.config)?,
            )
        })
        .await
        .unwrap_err();
    assert_eq!(partial.code(), ErrorCode::D1SqlInvalid);
    assert_eq!(history.latest_snapshot(account, resource).unwrap(), None);

    let checkpoint = coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history(),
        )
        .await
        .unwrap();
    assert_eq!(checkpoint.session_version, 2);
    let paths = D1Paths::open(storage.data_dir().root()).unwrap();
    let checkpoint_path = paths
        .resolve_snapshot_key(
            &checkpoint.snapshot_key,
            account,
            resource,
            checkpoint.session_version,
        )
        .unwrap();
    std::fs::remove_file(checkpoint_path).unwrap();

    let catalog = D1DatabaseRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    let path = D1Paths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&catalog.storage_key, account, resource)
        .unwrap();
    D1Engine::from_record(path, &catalog)
        .unwrap()
        .query(
            &D1Statement {
                sql: "INSERT INTO history VALUES ('committed')".to_owned(),
                params: vec![],
            },
            D1QueryLimits::query(&D1Config::default()).unwrap(),
        )
        .unwrap();

    let rows = coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| {
                context.engine.query(
                    &D1Statement {
                        sql: "SELECT value FROM history".to_owned(),
                        params: vec![],
                    },
                    D1QueryLimits::query(context.config)?,
                )
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rows.rows,
        vec![
            vec![D1Value::Text("prefix".to_owned())],
            vec![D1Value::Text("committed".to_owned())],
        ]
    );
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        2
    );
}

#[tokio::test]
async fn explicit_completed_history_retains_only_eight_unpinned_points() {
    let (_temp, storage, account, resource, coordinator) = fixture();
    for version in 1..=10 {
        coordinator
            .execute(account, resource, Duration::from_secs(2), true, |context| {
                context.mark_mutation();
                context.engine.exec(
                    "CREATE TABLE IF NOT EXISTS retained(value INTEGER); \
                     INSERT INTO retained VALUES (1)",
                    D1QueryLimits::query(context.config)?,
                )
            })
            .await
            .unwrap();
        let checkpoint = coordinator
            .execute(
                account,
                resource,
                Duration::from_secs(2),
                false,
                |context| context.checkpoint_completed_history(),
            )
            .await
            .unwrap();
        assert_eq!(checkpoint.session_version, version);
    }

    let history = D1SnapshotRepository::new(storage.db());
    for version in 1..=2 {
        assert_eq!(
            history
                .snapshot(account, resource, version)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceNotFound,
        );
    }
    for version in 3..=10 {
        assert_eq!(
            history
                .snapshot(account, resource, version)
                .unwrap()
                .session_version,
            version,
        );
    }
}

#[tokio::test]
async fn history_work_reclaims_an_expired_non_ingesting_transfer() {
    let (_temp, storage, account, resource, coordinator) = fixture();
    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history().map(|_| ()),
        )
        .await
        .unwrap();
    let transfer_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let history = D1SnapshotRepository::new(storage.db());
    history
        .create_transfer(&NewD1Transfer {
            id: &transfer_id,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Import,
            at_session_version: 0,
            filename: "expired-upload.sql",
            etag_md5: Some(&[2; 16]),
            token_fingerprint: &[3; 32],
            token_action: D1TransferAction::Upload,
            token_expires_at_ms: 1,
            now_ms: 0,
        })
        .unwrap();

    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history().map(|_| ()),
        )
        .await
        .unwrap();

    assert!(
        history
            .active_transfer(account, resource)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        history.transfer(account, &transfer_id).unwrap_err().code(),
        ErrorCode::ResourceNotFound,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloned_coordinator_has_one_lane_for_control_and_runtime_entries() {
    let (_temp, _storage, account, resource, coordinator) = fixture();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let first = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .execute(
                    account,
                    resource,
                    Duration::from_secs(2),
                    false,
                    move |_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                        Ok(())
                    },
                )
                .await
        }
    });
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let second_entered = Arc::new(AtomicBool::new(false));
    let second = tokio::spawn({
        let coordinator = coordinator.clone();
        let second_entered = second_entered.clone();
        async move {
            coordinator
                .execute(
                    account,
                    resource,
                    Duration::from_secs(2),
                    false,
                    move |_| {
                        second_entered.store(true, Ordering::Release);
                        Ok(())
                    },
                )
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!second_entered.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert!(second_entered.load(Ordering::Acquire));
}

#[tokio::test]
async fn pending_restore_replays_publication_and_completes_history_after_restart() {
    let (_temp, storage, account, resource, coordinator) = fixture();
    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history(),
        )
        .await
        .unwrap();
    coordinator
        .execute(account, resource, Duration::from_secs(2), true, |context| {
            context.mark_mutation();
            context.engine.exec(
                "CREATE TABLE discarded(value TEXT); INSERT INTO discarded VALUES ('new')",
                D1QueryLimits::query(context.config)?,
            )
        })
        .await
        .unwrap();
    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history(),
        )
        .await
        .unwrap();
    let history = D1SnapshotRepository::new(storage.db());
    let intent_id = uuid::Uuid::now_v7().hyphenated().to_string();
    history
        .prepare_restore(account, resource, &intent_id, 0, 1, &[9; 32], 20)
        .unwrap();
    drop(coordinator);

    let restarted = D1Coordinator::new(storage.clone(), ResourcePins::new(), D1Config::default());
    let version = restarted
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.engine.session_version(),
        )
        .await
        .unwrap();
    assert_eq!(version, 2);
    assert!(
        history
            .pending_restore(account, resource)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        2
    );
    let missing = restarted
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| {
                context.engine.query(
                    &D1Statement {
                        sql: "SELECT * FROM discarded".to_owned(),
                        params: vec![],
                    },
                    D1QueryLimits::query(context.config)?,
                )
            },
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code(), ErrorCode::D1SqlInvalid);
}

#[tokio::test]
async fn restart_replays_fenced_ingest_before_admitting_the_next_operation() {
    let (_temp, storage, account, resource, coordinator) = fixture();
    coordinator
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| context.checkpoint_completed_history().map(|_| ()),
        )
        .await
        .unwrap();
    let bytes = b"CREATE TABLE resumed(value TEXT); INSERT INTO resumed VALUES ('ok')";
    let etag: [u8; 16] = Md5::digest(bytes).into();
    let sha256: [u8; 32] = Sha256::digest(bytes).into();
    let session = uuid::Uuid::now_v7().hyphenated().to_string();
    let filename = format!("import-{resource}-{}.sql", hex::encode(etag));
    let history = D1SnapshotRepository::new(storage.db());
    history
        .create_transfer(&NewD1Transfer {
            id: &session,
            account_id: account,
            resource_id: resource,
            kind: D1TransferKind::Import,
            at_session_version: 0,
            filename: &filename,
            etag_md5: Some(&etag),
            token_fingerprint: &[7; 32],
            token_action: D1TransferAction::Upload,
            token_expires_at_ms: 10_000,
            now_ms: 100,
        })
        .unwrap();
    let paths = D1Paths::open(storage.data_dir().root()).unwrap();
    let key = paths
        .write_transfer(account, resource, &session, &filename, bytes)
        .unwrap();
    history
        .complete_upload(
            account,
            &session,
            &key,
            &etag,
            &sha256,
            bytes.len() as u64,
            110,
        )
        .unwrap();
    let catalog = D1DatabaseRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    let live = paths
        .resolve_storage_key(&catalog.storage_key, account, resource)
        .unwrap();
    let engine = D1Engine::from_record(live, &catalog).unwrap();
    let simulated_loss = engine
        .import_sql(
            std::str::from_utf8(bytes).unwrap(),
            D1QueryLimits::batch(&D1Config::default()).unwrap(),
            |result| {
                history
                    .begin_ingest(
                        account,
                        &session,
                        result.num_queries,
                        result.duration_ms,
                        result.rows_read,
                        result.rows_written,
                        result.size_after,
                        120,
                    )
                    .map(|_| ())?;
                Err(PlatformError::new(
                    ErrorCode::D1ResultUnknown,
                    "simulated response loss before live commit",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(simulated_loss.code(), ErrorCode::D1ResultUnknown);
    assert_eq!(engine.session_version().unwrap(), 0);
    drop(coordinator);

    let restarted = D1Coordinator::new(storage.clone(), ResourcePins::new(), D1Config::default());
    let rows = restarted
        .execute(
            account,
            resource,
            Duration::from_secs(2),
            false,
            |context| {
                context.engine.query(
                    &D1Statement {
                        sql: "SELECT value FROM resumed".to_owned(),
                        params: vec![],
                    },
                    D1QueryLimits::query(context.config)?,
                )
            },
        )
        .await
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Text("ok".to_owned())]]);
    let transfer = history.transfer(account, &session).unwrap();
    assert_eq!(transfer.state, D1TransferState::Complete);
    assert_eq!(transfer.result_session_version, Some(1));
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        1
    );
}
