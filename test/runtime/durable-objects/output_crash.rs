//! Process-death recovery of a committed Durable Object output intent.

use super::*;
use open_compute_core::QueueId;
use open_compute_service::queue_backend::QueueEnqueueHold;
use open_compute_storage::{QueueConfig, SchedulerStore};
use open_compute_workers::{CreateQueueOutcome, CreateQueueRequest, QueueController};

pub(super) fn open_scheduler(storage: &PlatformStorage) -> Arc<SchedulerStore> {
    Arc::new(SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5_000, 1).unwrap())
}

pub(super) fn create_queue(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    account_id: AccountId,
) -> (QueueId, ResourceId) {
    match QueueController::new(storage, scheduler)
        .create(&CreateQueueRequest {
            account_id,
            name: "do-output".to_owned(),
            config: QueueConfig {
                delivery_delay_seconds: 0,
                retention_seconds: 60,
                ..QueueConfig::default()
            },
            idempotency_key: "do-output-queue".to_owned(),
            request_id: RequestId::generate(),
            now_ms: 5,
        })
        .unwrap()
    {
        CreateQueueOutcome::Applied(result) => (
            result.queue.id,
            ResourceId::from_uuid(result.queue.id.as_uuid()).unwrap(),
        ),
        CreateQueueOutcome::Replay(_) => panic!("unexpected Queue create replay"),
    }
}

pub(super) struct Target<'a> {
    pub(super) queue: QueueId,
    pub(super) account: AccountId,
    pub(super) worker: WorkerId,
    pub(super) version: &'a VersionRecord,
    pub(super) generation: u64,
}

pub(super) async fn check(
    transport: &WorkerdTransport,
    supervisor: &WorkerdSupervisor,
    scheduler: &SchedulerStore,
    target: Target<'_>,
) {
    let Target {
        queue,
        account,
        worker,
        version,
        generation,
    } = target;
    let hold = QueueEnqueueHold::new();
    hold.install();
    hold.block_before();
    let mut pending = tokio::spawn({
        let transport = transport.clone();
        let version = version.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker,
                &version,
                generation,
                "/commit-output?name=output-crash-before",
            )
            .await
        }
    });
    tokio::select! {
        _ = hold.wait_seen(1) => {}
        result = &mut pending => panic!("output-before completed before enqueue hold: {result:?}"),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("output-before did not reach enqueue hold"),
    }
    assert_eq!(
        scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count,
        0,
        "enqueue must not run before the committed intent is recovered"
    );
    let old_pid = supervisor.snapshot().pid.unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(old_pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    hold.release_before();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    match tokio::time::timeout(Duration::from_secs(10), pending).await {
        Ok(Ok(response)) => assert_ne!(response.body, "", "{:?}", response.status),
        Ok(Err(_)) => {}
        Err(_) => panic!("output request from a dead runtime generation did not settle"),
    }
    let recovered = dispatch(
        transport,
        account,
        worker,
        version,
        generation,
        "/output-metrics?name=output-crash-before",
    )
    .await;
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    let metrics: serde_json::Value = serde_json::from_str(&recovered.body).unwrap();
    assert_eq!(metrics["backlogCount"], 1, "{metrics}");
    assert_eq!(
        scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count,
        1
    );

    hold.block_after();
    let mut pending = tokio::spawn({
        let transport = transport.clone();
        let version = version.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker,
                &version,
                generation,
                "/commit-output?name=output-crash-after",
            )
            .await
        }
    });
    tokio::select! {
        _ = hold.wait_seen(2) => {}
        result = &mut pending => panic!("output-after completed before enqueue hold: {result:?}"),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("output-after did not reach enqueue hold"),
    }
    let started = Instant::now();
    loop {
        let backlog = scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count;
        if backlog == 2 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "output-after was not durably admitted: backlog={backlog}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let old_pid = supervisor.snapshot().pid.unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(old_pid).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    hold.release_after();
    wait_pid_change(supervisor, old_pid, Duration::from_secs(30)).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), pending).await;
    let replayed = dispatch(
        transport,
        account,
        worker,
        version,
        generation,
        "/output-metrics?name=output-crash-after",
    )
    .await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    let replayed_metrics: serde_json::Value = serde_json::from_str(&replayed.body).unwrap();
    assert_eq!(replayed_metrics["backlogCount"], 2, "{replayed_metrics}");
    assert_eq!(
        scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count,
        2
    );

    let rolled_back = dispatch(
        transport,
        account,
        worker,
        version,
        generation,
        "/rollback-output?name=output-explicit-rollback",
    )
    .await;
    assert_eq!(rolled_back.status, 200, "{}", rolled_back.body);
    let rolled_back: serde_json::Value = serde_json::from_str(&rolled_back.body).unwrap();
    assert_eq!(rolled_back["stored"], false, "{rolled_back}");
    assert_eq!(rolled_back["metrics"]["backlogCount"], 3, "{rolled_back}");
    assert_eq!(
        scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count,
        3,
        "Cloudflare publishes output when transaction.rollback() returns normally"
    );

    let failed = dispatch(
        transport,
        account,
        worker,
        version,
        generation,
        "/failed-output?name=output-transaction-failed",
    )
    .await;
    assert_eq!(failed.status, 200, "{}", failed.body);
    let failed: serde_json::Value = serde_json::from_str(&failed.body).unwrap();
    assert_eq!(failed["stored"], false, "{failed}");
    assert_eq!(failed["metrics"]["backlogCount"], 3, "{failed}");
    assert_eq!(
        scheduler.queue_metrics(queue, 1, 1).unwrap().backlog_count,
        3,
        "a thrown transaction callback must discard its staged output"
    );
    QueueEnqueueHold::clear();
}
