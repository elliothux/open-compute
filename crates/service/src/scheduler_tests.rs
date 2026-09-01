use super::*;
use crate::metrics::MetricsRegistry;
use crate::runtime_bridge::WorkerdTransport;
use crate::runtime_bridge::{QueueDispatchResult, QueueRetryBatchResult, QueueRetryMessageResult};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    DURABLE_OBJECT_ID_BYTES, DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES, DeploymentId,
    DeterministicSchedulerClock, durable_object_namespace_prefix,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    AlarmProjection, ClaimedQueueBatch, ClaimedQueueMessage, QueueCompletionAction, QueueConfig,
    QueueContentType, QueueEnqueueRequest, QueueMessageInput, QueueProjection,
};
use std::path::Path;

#[test]
fn process_wall_floor_advances_but_never_moves_backwards() {
    let clock = DeterministicSchedulerClock::new(10_000);
    let floor = AtomicI64::new(clock.wall_time_ms());
    clock.set_wall_time_ms(1_000);
    assert_eq!(observe_wall_floor(&clock, &floor), 10_000);
    clock.set_wall_time_ms(20_000);
    assert_eq!(observe_wall_floor(&clock, &floor), 20_000);
    clock.set_wall_time_ms(5_000);
    assert_eq!(observe_wall_floor(&clock, &floor), 20_000);
}

#[tokio::test]
async fn dispatch_timeout_uses_the_scheduler_clock() {
    let clock = Arc::new(DeterministicSchedulerClock::new(10_000));
    let timeout = tokio::spawn({
        let clock = clock.clone();
        async move {
            scheduler_timeout(
                clock.as_ref(),
                Duration::from_secs(60),
                std::future::pending::<()>(),
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    assert_eq!(clock.pending_timer_count(), 1);
    clock.advance_monotonic(Duration::from_secs(60));
    assert_eq!(timeout.await.unwrap(), Err(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kernel_run_claims_releases_and_shuts_down_without_polling() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(temp.path()), &SystemClock).unwrap());
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 10).unwrap());
    let namespace = ResourceId::generate();
    let mut object_bytes = [7; DURABLE_OBJECT_ID_BYTES];
    object_bytes[..DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES]
        .copy_from_slice(&durable_object_namespace_prefix(namespace));
    let object_id = DurableObjectId::for_namespace(object_bytes, namespace).unwrap();
    store
        .upsert_alarm(
            &AlarmProjection {
                namespace_resource_id: namespace,
                object_id,
                object_generation: 1,
                row_token: "coverage-token-01".to_owned(),
                due_at_ms: 10,
                target_deployment_id: DeploymentId::generate(),
                execution_generation: 1,
                retry_count: 0,
            },
            10,
        )
        .unwrap();

    let clock = Arc::new(DeterministicSchedulerClock::new(10));
    let fault_count = Arc::new(AtomicUsize::new(0));
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let scheduler = Arc::new(
        SchedulerService::new(
            store.clone(),
            storage,
            transport,
            {
                let mut config = SchedulerConfig::default();
                config.pools.alarm.claim_batch = 1;
                config
            },
            open_compute_core::WorkflowsConfig::default(),
            clock,
        )
        .with_fault_hook({
            let fault_count = fault_count.clone();
            Arc::new(move |_| {
                fault_count.fetch_add(1, Ordering::Relaxed);
            })
        }),
    );
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(shutdown_rx));

    tokio::time::timeout(Duration::from_secs(5), async {
        while store.summary(10).unwrap().scheduled != 0 || fault_count.load(Ordering::Relaxed) != 1
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(store.summary(10).unwrap().scheduled, 0);
    assert_eq!(fault_count.load(Ordering::Relaxed), 1);
    scheduler.pause_kind(SchedulerKind::Alarm).unwrap();
    assert_eq!(
        scheduler.inspect().unwrap().pools[0].state,
        SchedulerPoolState::Paused
    );
    scheduler.resume_kind(SchedulerKind::Alarm).unwrap();

    shutdown_tx.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    assert_eq!(scheduler.global_in_flight.load(Ordering::Acquire), 0);
    assert_eq!(scheduler.alarm_in_flight.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn kernel_run_reports_disabled_and_globally_paused_pools() {
    for disabled in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            PlatformStorage::bootstrap(&storage_config(temp.path()), &SystemClock).unwrap(),
        );
        let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
        let store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 10).unwrap());
        let mut config = SchedulerConfig::default();
        if disabled {
            let mut pools = open_compute_core::SchedulerPoolsConfig::default();
            pools.alarm.enabled = false;
            pools.queue.enabled = false;
            pools.cron.enabled = false;
            pools.workflow.enabled = false;
            config.pools = pools;
        }
        let scheduler = Arc::new(SchedulerService::new(
            store,
            storage,
            WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
            config,
            open_compute_core::WorkflowsConfig::default(),
            Arc::new(DeterministicSchedulerClock::new(10)),
        ));
        if !disabled {
            scheduler.pause();
        }
        let (shutdown, shutdown_rx) = watch::channel(false);
        let kernel = tokio::spawn(scheduler.clone().run(shutdown_rx));
        let expected = if disabled {
            SchedulerPoolState::Disabled
        } else {
            SchedulerPoolState::Paused
        };
        for _ in 0..10_000 {
            if scheduler
                .inspect()
                .unwrap()
                .pools
                .iter()
                .all(|pool| pool.state == expected)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            scheduler
                .inspect()
                .unwrap()
                .pools
                .iter()
                .all(|pool| pool.state == expected)
        );
        shutdown.send(true).unwrap();
        kernel.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn scheduler_helpers_cover_all_fixed_states_and_completion_results() {
    assert_eq!(infrastructure_error_class(ErrorCode::SchedulerBusy), 1);
    assert_eq!(
        infrastructure_error_class(ErrorCode::SchedulerUnavailable),
        2
    );
    assert_eq!(
        infrastructure_error_class(ErrorCode::SchedulerInternalProtocolError),
        3
    );
    assert_eq!(infrastructure_error_class(ErrorCode::LimitInvalid), 4);
    for state in [
        SchedulerPoolState::Ready,
        SchedulerPoolState::Paused,
        SchedulerPoolState::Backoff,
        SchedulerPoolState::CircuitOpen,
        SchedulerPoolState::Disabled,
    ] {
        assert_eq!(decode_pool_state(encode_pool_state(state)), state);
    }
    assert_eq!(decode_pool_state(u8::MAX), SchedulerPoolState::Ready);
    assert_eq!(
        scheduler_task_failed().code(),
        ErrorCode::SchedulerUnavailable
    );

    let mut admission = AdmissionTracker::new(2, [2; SchedulerKind::ALL.len()]);
    assert!(admission.reserve(SchedulerKind::Alarm, 1));
    admission.release(SchedulerKind::Alarm, 1);
    assert_eq!(admission.global_in_flight(), 0);
    assert!(admission.reserve(SchedulerKind::Workflow, 1));
    let task = tokio::spawn(async {
        panic!("expected test task failure");
        #[allow(unreachable_code)]
        SchedulerKind::Workflow
    });
    let task_id = task.id();
    let mut kinds = std::collections::HashMap::from([(task_id, SchedulerKind::Workflow)]);
    let failed = task.await.map(|kind| (task_id, kind));
    let kind = completed_kind(failed, &mut kinds).unwrap();
    assert_eq!(kind, SchedulerKind::Workflow);
    admission.release(kind, 1);
    assert_eq!(admission.pool_in_flight(SchedulerKind::Workflow), 0);
}

#[test]
fn queue_disposition_precedence_and_membership_are_exact() {
    let first = open_compute_core::QueueMessageId::generate();
    let second = open_compute_core::QueueMessageId::generate();
    let batch = ClaimedQueueBatch {
        id: open_compute_core::QueueBatchId::generate(),
        account_id: AccountId::generate(),
        queue_id: QueueId::generate(),
        consumer_id: QueueConsumerId::generate(),
        consumer_generation: 1,
        deployment_id: DeploymentId::generate(),
        worker_id: WorkerId::generate(),
        execution_generation: 1,
        entrypoint: None,
        retry_delay_seconds: 5,
        claim_token: [7; 32],
        claim_until_ms: 1_000,
        messages: [first, second]
            .into_iter()
            .map(|id| ClaimedQueueMessage {
                id,
                enqueued_at_ms: 10,
                delivery_attempt: 1,
                content_type: QueueContentType::Text,
                body: b"body".to_vec(),
            })
            .collect(),
    };
    let response = |outcome: &str| QueueDispatchResult {
        outcome: outcome.to_owned(),
        ack_all: false,
        retry_batch: QueueRetryBatchResult {
            retry: false,
            delay_seconds: None,
        },
        explicit_acks: Vec::new(),
        retry_messages: Vec::new(),
    };

    let success = queue::resolve_queue_disposition(&batch, &response("ok"), true).unwrap();
    assert!(
        success
            .iter()
            .all(|decision| decision.action == QueueCompletionAction::Ack)
    );
    let failure = queue::resolve_queue_disposition(&batch, &response("exception"), false).unwrap();
    assert!(
        failure.iter().all(|decision| {
            decision.action == QueueCompletionAction::Retry { delay_seconds: 5 }
        })
    );

    let explicit = QueueDispatchResult {
        explicit_acks: vec![first.to_string()],
        retry_messages: vec![QueueRetryMessageResult {
            msg_id: second.to_string(),
            delay_seconds: Some(9),
        }],
        ..response("ok")
    };
    let decisions = queue::resolve_queue_disposition(&batch, &explicit, true).unwrap();
    assert_eq!(decisions[0].action, QueueCompletionAction::Ack);
    assert_eq!(
        decisions[1].action,
        QueueCompletionAction::Retry { delay_seconds: 9 }
    );

    let batch_retry = QueueDispatchResult {
        retry_batch: QueueRetryBatchResult {
            retry: true,
            delay_seconds: Some(11),
        },
        ..response("exception")
    };
    assert!(
        queue::resolve_queue_disposition(&batch, &batch_retry, false)
            .unwrap()
            .iter()
            .all(|decision| decision.action == QueueCompletionAction::Retry { delay_seconds: 11 })
    );

    for invalid in [
        QueueDispatchResult {
            ack_all: true,
            retry_batch: QueueRetryBatchResult {
                retry: true,
                delay_seconds: None,
            },
            ..response("ok")
        },
        QueueDispatchResult {
            explicit_acks: vec![open_compute_core::QueueMessageId::generate().to_string()],
            ..response("ok")
        },
        QueueDispatchResult {
            explicit_acks: vec![first.to_string(), first.to_string()],
            ..response("ok")
        },
        QueueDispatchResult {
            explicit_acks: vec![first.to_string()],
            retry_messages: vec![QueueRetryMessageResult {
                msg_id: first.to_string(),
                delay_seconds: None,
            }],
            ..response("ok")
        },
        QueueDispatchResult {
            retry_messages: vec![QueueRetryMessageResult {
                msg_id: second.to_string(),
                delay_seconds: Some(-1),
            }],
            ..response("ok")
        },
        QueueDispatchResult {
            retry_batch: QueueRetryBatchResult {
                retry: true,
                delay_seconds: Some(-1),
            },
            ..response("exception")
        },
    ] {
        assert_eq!(
            queue::resolve_queue_disposition(&batch, &invalid, true)
                .unwrap_err()
                .code(),
            ErrorCode::QueueDispositionInvalid
        );
    }
}

#[tokio::test]
async fn queue_retention_adapter_observes_pause_metrics_and_successful_sweeps() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(temp.path()), &SystemClock).unwrap());
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 1).unwrap());
    let queue_id = QueueId::generate();
    let account_id = storage.identity().default_account_id;
    let config = QueueConfig {
        retention_seconds: 60,
        max_backlog_bytes: 1024,
        ..QueueConfig::default()
    };
    store
        .create_queue_projection(&QueueProjection {
            queue_id,
            account_id,
            lifecycle_generation: 1,
            config_generation: 1,
            config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"expired".to_vec(),
                    delay_seconds: None,
                }],
            },
            1,
        )
        .unwrap();

    let clock = Arc::new(DeterministicSchedulerClock::new(60_001));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let scheduler = Arc::new(
        SchedulerService::new(
            store.clone(),
            storage,
            transport,
            SchedulerConfig::default(),
            open_compute_core::WorkflowsConfig::default(),
            clock,
        )
        .with_metrics(metrics),
    );

    scheduler.pause();
    assert_eq!(scheduler.poll_queue_once().unwrap(), 0);
    scheduler.resume();
    scheduler.pause_kind(SchedulerKind::Queue).unwrap();
    assert_eq!(scheduler.queue_pool_state(), SchedulerPoolState::Paused);
    assert_eq!(scheduler.poll_queue_once().unwrap(), 0);
    scheduler.resume_kind(SchedulerKind::Queue).unwrap();
    let (shutdown, shutdown_rx) = watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(shutdown_rx));
    for _ in 0..10_000 {
        if store.queue_backlog_totals().unwrap() == (0, 0) {
            break;
        }
        tokio::task::yield_now().await;
    }
    shutdown.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    assert_eq!(store.queue_backlog_totals().unwrap(), (0, 0));

    scheduler.set_queue_pool_state(SchedulerPoolState::Backoff);
    assert_eq!(scheduler.queue_pool_state(), SchedulerPoolState::Backoff);
    assert!(scheduler.run_queue_maintenance(60_001, 0).await.is_err());
    assert_eq!(scheduler.queue_pool_state(), SchedulerPoolState::Backoff);
    scheduler.run_queue_maintenance(60_001, 1).await.unwrap();
    assert_eq!(scheduler.queue_in_flight.load(Ordering::Acquire), 0);
    assert_eq!(scheduler.queue_pool_state(), SchedulerPoolState::Backoff);

    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Bytes,
                    body: b"corrupt".to_vec(),
                    delay_seconds: None,
                }],
            },
            1,
        )
        .unwrap();
    let connection = rusqlite::Connection::open(&scheduler_path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER queue_messages_immutable_guard;
             DROP TRIGGER queue_messages_transition_guard;
             PRAGMA ignore_check_constraints = ON;
             UPDATE queue_messages SET body_bytes = -1;",
        )
        .unwrap();
    drop(connection);
    assert!(scheduler.run_queue_maintenance(60_001, 1).await.is_err());
    assert_eq!(scheduler.queue_pool_state(), SchedulerPoolState::Backoff);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(shutdown_rx));
    for _ in 0..10_000 {
        if scheduler.queue_pool_state() == SchedulerPoolState::CircuitOpen {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        scheduler.queue_pool_state(),
        SchedulerPoolState::CircuitOpen
    );
    shutdown.send(true).unwrap();
    kernel.await.unwrap().unwrap();
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.join("data"),
        master_key_file: root.join("data/keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}
