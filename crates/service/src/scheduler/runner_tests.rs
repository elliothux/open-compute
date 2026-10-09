use super::*;
use open_compute_core::DeterministicSchedulerClock;
use open_compute_storage::scheduler::SchedulerWakeSignal;

#[tokio::test]
async fn saturated_dispatch_waits_for_completion_without_delaying_maintenance() {
    let clock = Arc::new(DeterministicSchedulerClock::new(10_000));
    let wake = WakeCoordinator::new(
        Arc::new(SchedulerWakeSignal::default()),
        clock.clone(),
        Duration::from_secs(60),
    );
    let now = clock.monotonic_now();
    let mut input = DeadlineInputs {
        repair_deadline: now + Duration::from_secs(30),
        now_ms: 10_000,
        next_due_at_ms: [Some(9_000), Some(9_000), None, Some(9_000), Some(9_000)],
        runnable: [true; SchedulerKind::ALL.len()],
        retry_at: [None; SchedulerKind::ALL.len()],
    };
    let mut admission = AdmissionTracker::new(1, [1; SchedulerKind::ALL.len()]);
    assert!(admission.reserve(SchedulerKind::Queue, 1));
    let deadlines = SchedulerService::deadlines(&wake, &input, &admission);
    let waiter = tokio::spawn({
        let wake = wake.clone();
        let observed = wake.generation();
        async move { wake.wait(observed, &deadlines).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(clock.pending_timer_count(), 1);
    assert!(!waiter.is_finished());
    // Completion frees admission and immediately makes overdue dispatch actionable.
    admission.release(SchedulerKind::Queue, 1);
    let deadlines = SchedulerService::deadlines(&wake, &input, &admission);
    assert_eq!(
        wake.wait(wake.generation(), &deadlines).await,
        WakeReason::Due
    );
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());

    // A full pool does not suppress another pool that still has global admission.
    admission = AdmissionTracker::new(2, [1; SchedulerKind::ALL.len()]);
    assert!(admission.reserve(SchedulerKind::Queue, 1));
    input.next_due_at_ms = [None, Some(9_000), None, None, None];
    assert!(
        SchedulerService::deadlines(&wake, &input, &admission)
            .iter()
            .all(|deadline| deadline.at > now)
    );
    input.next_due_at_ms[0] = Some(9_000);
    let deadlines = SchedulerService::deadlines(&wake, &input, &admission);
    assert_eq!(
        wake.wait(wake.generation(), &deadlines).await,
        WakeReason::Due
    );

    // Retention, future lease/DLQ timers, retry and repair do not need a dispatch slot.
    assert!(admission.reserve(SchedulerKind::Alarm, 1));
    input.next_due_at_ms = [None, Some(10_010), Some(9_000), None, None];
    input.retry_at[SchedulerKind::Queue.index()] = Some(now + Duration::from_millis(5));
    let deadlines = SchedulerService::deadlines(&wake, &input, &admission);
    assert_eq!(
        wake.wait(wake.generation(), &deadlines).await,
        WakeReason::Due
    );
    assert!(deadlines.iter().any(|deadline| {
        deadline.reason == WakeReason::Due && deadline.at == now + Duration::from_millis(10)
    }));
    assert!(deadlines.iter().any(|deadline| {
        deadline.reason == WakeReason::Backoff && deadline.at == now + Duration::from_millis(5)
    }));
    assert!(deadlines.iter().any(|deadline| {
        deadline.reason == WakeReason::Repair && deadline.at == input.repair_deadline
    }));
}
