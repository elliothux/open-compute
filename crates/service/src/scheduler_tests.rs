use super::*;
use crate::runtime_bridge::WorkerdTransport;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    DURABLE_OBJECT_ID_BYTES, DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES, DeploymentId,
    DeterministicSchedulerClock, durable_object_namespace_prefix,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::AlarmProjection;
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
            SchedulerConfig {
                claim_batch: 1,
                ..SchedulerConfig::default()
            },
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

    for _ in 0..10_000 {
        if store.summary(10).unwrap().scheduled == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
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
    let global = AtomicUsize::new(0);
    let alarm = AtomicUsize::new(0);
    assert!(admission.reserve(SchedulerKind::Alarm, 1));
    release_completed(&Ok(SchedulerKind::Alarm), &mut admission, &global, &alarm);
    assert_eq!(global.load(Ordering::Acquire), 0);
    assert!(admission.reserve(SchedulerKind::Alarm, 1));
    let failed = tokio::spawn(async { panic!("expected test task failure") }).await;
    release_completed(&failed, &mut admission, &global, &alarm);
    assert_eq!(alarm.load(Ordering::Acquire), 0);
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
