use super::*;
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, RequestId, SystemClock};
use open_compute_storage::{D1QueryLimits, D1Statement, D1Value};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
};
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
async fn every_entry_snapshots_initial_mutation_and_lost_response_head() {
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
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        0
    );

    coordinator
        .execute(account, resource, Duration::from_secs(2), true, |context| {
            context.mark_mutation();
            context.engine.exec(
                "CREATE TABLE history(value TEXT)",
                D1QueryLimits::query(context.config)?,
            )
        })
        .await
        .unwrap();
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        1
    );

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
    assert_eq!(rows.rows, vec![vec![D1Value::Text("committed".to_owned())]]);
    assert_eq!(
        history
            .latest_snapshot(account, resource)
            .unwrap()
            .unwrap()
            .session_version,
        2
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
