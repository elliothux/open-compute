//! Fair multi-pool scheduler run loop and one-shot polling.

use super::*;

impl SchedulerService {
    /// Run generation-safe claim/repair loops, then boundedly drain in-flight dispatches.
    pub async fn run(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let queue = self.config.pool(SchedulerKind::Queue);
        let cron = self.config.pool(SchedulerKind::Cron);
        let workflow = self.config.pool(SchedulerKind::Workflow);
        let pool_caps = SchedulerKind::ALL.map(|kind| {
            usize::try_from(self.config.pool(kind).max_in_flight).unwrap_or(usize::MAX)
        });
        let weights = SchedulerKind::ALL.map(|kind| self.config.pool(kind).weight);
        let mut admission = AdmissionTracker::new(
            usize::try_from(self.config.max_in_flight).unwrap_or(usize::MAX),
            pool_caps,
        );
        let mut selector = FairSelector::new(weights);
        let mut backoff = InfrastructureBackoff::new(
            self.observed_wall_time_ms().unsigned_abs(),
            Duration::from_millis(25),
            Duration::from_secs(5),
        );
        let mut pool = PoolRuntime::ready();
        let mut queue_pool = PoolRuntime::ready();
        let mut cron_pool = PoolRuntime::ready();
        let mut workflow_pool = PoolRuntime::ready();
        let mut repair_deadline = self.clock.monotonic_now();
        let mut dispatches = JoinSet::new();
        let mut version_reconcile = JoinSet::new();
        let mut dispatch_kinds = std::collections::HashMap::new();

        loop {
            while let Some(result) = version_reconcile.try_join_next() {
                if !matches!(result, Ok(Ok(()))) {
                    tracing::warn!(
                        code = "WORKFLOW_RUNTIME_UNAVAILABLE",
                        "Workflow version reconciliation failed"
                    );
                }
            }
            while let Some(completed) = dispatches.try_join_next_with_id() {
                self.release_completed(
                    completed_kind(completed, &mut dispatch_kinds)?,
                    &mut admission,
                );
            }
            if *shutdown.borrow() {
                break;
            }

            let observed_generation = self.wake.generation();
            let now_mono = self.clock.monotonic_now();
            pool.refresh_deadline(now_mono);
            queue_pool.refresh_deadline(now_mono);
            cron_pool.refresh_deadline(now_mono);
            workflow_pool.refresh_deadline(now_mono);
            if self.workflow_pool_state() == SchedulerPoolState::CircuitOpen {
                workflow_pool.permanent_failure();
            } else if workflow_pool.state() == SchedulerPoolState::CircuitOpen
                && self.workflow_pool_state() == SchedulerPoolState::Ready
            {
                workflow_pool.probe_succeeded();
            }
            if !workflow.enabled {
                self.set_workflow_pool_state(SchedulerPoolState::Disabled);
            } else if self.is_paused() || self.workflow_paused.load(Ordering::Acquire) {
                self.set_workflow_pool_state(SchedulerPoolState::Paused);
            } else {
                self.set_workflow_pool_state(workflow_pool.state());
            }
            if pool.state() == SchedulerPoolState::CircuitOpen
                && self.alarm_pool_state() == SchedulerPoolState::Ready
            {
                pool.probe_succeeded();
            }
            if !alarm.enabled {
                self.set_alarm_pool_state(SchedulerPoolState::Disabled);
            } else if self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
                self.set_alarm_pool_state(SchedulerPoolState::Paused);
            } else {
                self.set_alarm_pool_state(pool.state());
            }
            if queue_pool.state() == SchedulerPoolState::CircuitOpen
                && self.queue_pool_state() == SchedulerPoolState::Ready
            {
                queue_pool.probe_succeeded();
            }
            if cron_pool.state() == SchedulerPoolState::CircuitOpen
                && self.cron_pool_state() == SchedulerPoolState::Ready
            {
                cron_pool.probe_succeeded();
            }
            if !queue.enabled {
                self.set_queue_pool_state(SchedulerPoolState::Disabled);
            } else if self.is_paused() || self.queue_paused.load(Ordering::Acquire) {
                self.set_queue_pool_state(SchedulerPoolState::Paused);
            } else {
                self.set_queue_pool_state(queue_pool.state());
            }
            if !cron.enabled {
                self.set_cron_pool_state(SchedulerPoolState::Disabled);
            } else if self.is_paused() || self.cron_paused.load(Ordering::Acquire) {
                self.set_cron_pool_state(SchedulerPoolState::Paused);
            } else {
                self.set_cron_pool_state(cron_pool.state());
            }

            if now_mono >= repair_deadline {
                if version_reconcile.is_empty() && !self.is_paused() && workflow.enabled {
                    let service = self.clone();
                    version_reconcile
                        .spawn(async move { service.reconcile_workflow_versions(1).await });
                }
                if let Err(error) = self.repair_workflows(32) {
                    if error.code() == ErrorCode::WorkflowInvariantViolation {
                        workflow_pool.permanent_failure();
                        self.set_workflow_pool_state(workflow_pool.state());
                    }
                    tracing::warn!(
                        code = error.code().as_str(),
                        "Workflow reconciliation failed"
                    );
                }
                if let Err(error) = self.repair_once().await {
                    tracing::warn!(
                        code = error.code().as_str(),
                        "scheduler alarm repair pass failed"
                    );
                }
                repair_deadline = self
                    .clock
                    .monotonic_deadline(Duration::from_millis(self.config.repair_interval_ms));
            }

            let now_ms = self.observed_wall_time_ms();
            let summary = self.pool_summary(now_ms).await?;
            let queue_retention = self.store.queue_workload_summary(now_ms)?;
            let queue_summary = self.store.queue_consumer_workload_summary(now_ms)?;
            let cron_summary = self.store.cron_workload_summary(now_ms)?;
            let workflow_summary = self.store.workflow_workload_summary(now_ms)?;
            set_atomic_option_i64(
                &self.next_wake_at_ms,
                minimum_timestamp([
                    summary.next_due_at_ms,
                    queue_retention.next_due_at_ms,
                    queue_summary.next_due_at_ms,
                    cron_summary.next_due_at_ms,
                    workflow_summary.next_due_at_ms,
                ]),
            );
            if let Some(metrics) = &self.metrics {
                metrics.observe_scheduler_workload(SchedulerKind::Alarm, summary, now_ms);
                metrics.observe_scheduler_workload(SchedulerKind::Queue, queue_summary, now_ms);
                metrics.observe_scheduler_workload(SchedulerKind::Cron, cron_summary, now_ms);
                metrics.observe_scheduler_workload(
                    SchedulerKind::Workflow,
                    workflow_summary,
                    now_ms,
                );
                metrics.set_cron_lag(
                    cron_summary
                        .oldest_due_at_ms
                        .map_or(0.0, |due| now_ms.saturating_sub(due).max(0) as f64 / 1000.0),
                );
                if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                    metrics.set_queue_backlog(messages, bytes);
                }
            }
            let mut queue_runnable = queue.enabled
                && !self.is_paused()
                && !self.queue_paused.load(Ordering::Acquire)
                && queue_pool.state() == SchedulerPoolState::Ready;
            let dlq_pending_due = self.store.queue_dlq_pending_due(now_ms)?;
            if queue_runnable && (queue_retention.ready > 0 || dlq_pending_due > 0) {
                match self.run_queue_maintenance(now_ms, queue.claim_batch).await {
                    Ok(()) => {
                        backoff.reset(SchedulerKind::Queue);
                        queue_pool.probe_succeeded();
                    }
                    Err(error)
                        if permanent_pool_error(error.code())
                            || matches!(
                                error.code(),
                                ErrorCode::QueueInvariantViolation | ErrorCode::SchedulerCorrupt
                            ) =>
                    {
                        queue_pool.permanent_failure();
                        self.set_queue_pool_state(queue_pool.state());
                        queue_runnable = false;
                    }
                    Err(error) => {
                        let delay = backoff.fail(
                            SchedulerKind::Queue,
                            infrastructure_error_class(error.code()),
                        );
                        queue_pool.transient_failure(self.clock.monotonic_deadline(delay));
                        self.set_queue_pool_state(queue_pool.state());
                        queue_runnable = false;
                        tracing::warn!(
                            code = error.code().as_str(),
                            "Queue maintenance entered bounded backoff"
                        );
                    }
                }
            }
            let pool_runnable = alarm.enabled
                && !self.is_paused()
                && !self.alarm_paused.load(Ordering::Acquire)
                && pool.state() == SchedulerPoolState::Ready;
            let cron_runnable = cron.enabled
                && !self.is_paused()
                && !self.cron_paused.load(Ordering::Acquire)
                && cron_pool.state() == SchedulerPoolState::Ready;
            let workflow_runnable = workflow.enabled
                && !self.is_paused()
                && !self.workflow_paused.load(Ordering::Acquire)
                && workflow_pool.state() == SchedulerPoolState::Ready;
            if admission.available_global() > 0 {
                let mut ready = [false; SchedulerKind::ALL.len()];
                ready[SchedulerKind::Alarm.index()] =
                    pool_runnable && (summary.ready > 0 || summary.expired > 0);
                ready[SchedulerKind::Queue.index()] =
                    queue_runnable && (queue_summary.ready > 0 || queue_summary.expired > 0);
                ready[SchedulerKind::Cron.index()] =
                    cron_runnable && (cron_summary.ready > 0 || cron_summary.expired > 0);
                ready[SchedulerKind::Workflow.index()] = workflow_runnable
                    && (workflow_summary.ready > 0 || workflow_summary.expired > 0);
                let selected = selector.select(
                    ready,
                    admission.available_pools(),
                    admission.available_global(),
                );
                let selected_alarm = selected
                    .iter()
                    .filter(|kind| **kind == SchedulerKind::Alarm)
                    .count()
                    .min(usize::try_from(alarm.claim_batch).unwrap_or(usize::MAX));
                if selected_alarm > 0 {
                    match self
                        .claim(u32::try_from(selected_alarm).unwrap_or(u32::MAX))
                        .await
                    {
                        Ok(jobs) => {
                            backoff.reset(SchedulerKind::Alarm);
                            let unused = selected_alarm.saturating_sub(jobs.len());
                            selector.refund(SchedulerKind::Alarm, unused);
                            if !jobs.is_empty() {
                                if !admission.reserve(SchedulerKind::Alarm, jobs.len()) {
                                    return Err(scheduler_task_failed());
                                }
                                self.store_admission_metrics(&admission);
                                for job in jobs {
                                    let service = self.clone();
                                    let handle = dispatches.spawn(async move {
                                        service.dispatch_one(job).await;
                                        SchedulerKind::Alarm
                                    });
                                    dispatch_kinds.insert(handle.id(), SchedulerKind::Alarm);
                                }
                            }
                        }
                        Err(error) if error.code() == ErrorCode::SchedulerCorrupt => {
                            return Err(error);
                        }
                        Err(error) if permanent_pool_error(error.code()) => {
                            pool.permanent_failure();
                            self.set_alarm_pool_state(pool.state());
                            self.observe_pool_health(pool.state());
                        }
                        Err(error) => {
                            let delay = backoff.fail(
                                SchedulerKind::Alarm,
                                infrastructure_error_class(error.code()),
                            );
                            pool.transient_failure(self.clock.monotonic_deadline(delay));
                            self.set_alarm_pool_state(pool.state());
                            tracing::warn!(
                                code = error.code().as_str(),
                                "scheduler due claim entered bounded backoff"
                            );
                        }
                    }
                }
                let selected_queue = selected
                    .iter()
                    .filter(|kind| **kind == SchedulerKind::Queue)
                    .count()
                    .min(usize::try_from(queue.claim_batch).unwrap_or(usize::MAX));
                if selected_queue > 0 {
                    match self
                        .claim_queue_consumers(u32::try_from(selected_queue).unwrap_or(u32::MAX))
                        .await
                    {
                        Ok(batches) => {
                            backoff.reset(SchedulerKind::Queue);
                            queue_pool.probe_succeeded();
                            selector.refund(
                                SchedulerKind::Queue,
                                selected_queue.saturating_sub(batches.len()),
                            );
                            if !batches.is_empty() {
                                if !admission.reserve(SchedulerKind::Queue, batches.len()) {
                                    return Err(scheduler_task_failed());
                                }
                                self.store_admission_metrics(&admission);
                                for batch in batches {
                                    let service = self.clone();
                                    let handle = dispatches.spawn(async move {
                                        service.dispatch_queue_batch(batch).await;
                                        SchedulerKind::Queue
                                    });
                                    dispatch_kinds.insert(handle.id(), SchedulerKind::Queue);
                                }
                                self.set_queue_pool_state(queue_pool.state());
                            }
                        }
                        Err(error) if error.code() == ErrorCode::SchedulerCorrupt => {
                            return Err(error);
                        }
                        Err(error) if permanent_pool_error(error.code()) => {
                            queue_pool.permanent_failure();
                            self.set_queue_pool_state(queue_pool.state());
                        }
                        Err(error) => {
                            let delay = backoff.fail(
                                SchedulerKind::Queue,
                                infrastructure_error_class(error.code()),
                            );
                            queue_pool.transient_failure(self.clock.monotonic_deadline(delay));
                            self.set_queue_pool_state(queue_pool.state());
                            tracing::warn!(
                                code = error.code().as_str(),
                                "Queue consumer claim entered bounded backoff"
                            );
                        }
                    }
                }
                let selected_cron = selected
                    .iter()
                    .filter(|kind| **kind == SchedulerKind::Cron)
                    .count()
                    .min(usize::try_from(cron.claim_batch).unwrap_or(usize::MAX));
                if selected_cron > 0 {
                    match self
                        .claim_cron(u32::try_from(selected_cron).unwrap_or(u32::MAX))
                        .await
                    {
                        Ok(runs) => {
                            backoff.reset(SchedulerKind::Cron);
                            cron_pool.probe_succeeded();
                            selector.refund(
                                SchedulerKind::Cron,
                                selected_cron.saturating_sub(runs.len()),
                            );
                            if !runs.is_empty() {
                                if !admission.reserve(SchedulerKind::Cron, runs.len()) {
                                    return Err(scheduler_task_failed());
                                }
                                self.store_admission_metrics(&admission);
                                for run in runs {
                                    let service = self.clone();
                                    let handle = dispatches.spawn(async move {
                                        service.dispatch_cron_run(run).await;
                                        SchedulerKind::Cron
                                    });
                                    dispatch_kinds.insert(handle.id(), SchedulerKind::Cron);
                                }
                                self.set_cron_pool_state(cron_pool.state());
                            }
                        }
                        Err(error) if error.code() == ErrorCode::SchedulerCorrupt => {
                            return Err(error);
                        }
                        Err(error) if permanent_pool_error(error.code()) => {
                            cron_pool.permanent_failure();
                            self.set_cron_pool_state(cron_pool.state());
                        }
                        Err(error) => {
                            let delay = backoff.fail(
                                SchedulerKind::Cron,
                                infrastructure_error_class(error.code()),
                            );
                            cron_pool.transient_failure(self.clock.monotonic_deadline(delay));
                            self.set_cron_pool_state(cron_pool.state());
                            tracing::warn!(
                                code = error.code().as_str(),
                                "Cron claim entered bounded backoff"
                            );
                        }
                    }
                }
                let selected_workflow = selected
                    .iter()
                    .filter(|kind| **kind == SchedulerKind::Workflow)
                    .count()
                    .min(usize::try_from(workflow.claim_batch).unwrap_or(usize::MAX));
                if selected_workflow > 0 {
                    match self
                        .claim_workflows(u32::try_from(selected_workflow).unwrap_or(u32::MAX))
                        .await
                    {
                        Ok(runs) => {
                            backoff.reset(SchedulerKind::Workflow);
                            workflow_pool.probe_succeeded();
                            selector.refund(
                                SchedulerKind::Workflow,
                                selected_workflow.saturating_sub(runs.len()),
                            );
                            if !runs.is_empty() {
                                if !admission.reserve(SchedulerKind::Workflow, runs.len()) {
                                    return Err(scheduler_task_failed());
                                }
                                self.store_admission_metrics(&admission);
                                for run in runs {
                                    let service = self.clone();
                                    let handle = dispatches.spawn(async move {
                                        service.dispatch_workflow_run(run).await;
                                        SchedulerKind::Workflow
                                    });
                                    dispatch_kinds.insert(handle.id(), SchedulerKind::Workflow);
                                }
                                self.set_workflow_pool_state(workflow_pool.state());
                            }
                        }
                        Err(error) if error.code() == ErrorCode::SchedulerCorrupt => {
                            return Err(error);
                        }
                        Err(error)
                            if permanent_pool_error(error.code())
                                || error.code() == ErrorCode::WorkflowInvariantViolation =>
                        {
                            workflow_pool.permanent_failure();
                            self.set_workflow_pool_state(workflow_pool.state());
                        }
                        Err(error) => {
                            let delay = backoff.fail(
                                SchedulerKind::Workflow,
                                infrastructure_error_class(error.code()),
                            );
                            workflow_pool.transient_failure(self.clock.monotonic_deadline(delay));
                            self.set_workflow_pool_state(workflow_pool.state());
                            tracing::warn!(
                                code = error.code().as_str(),
                                "Workflow claim entered bounded backoff"
                            );
                        }
                    }
                }
            }

            let mut deadlines = vec![WakeDeadline {
                at: repair_deadline,
                reason: WakeReason::Repair,
            }];
            if pool_runnable && let Some(next_due_at_ms) = summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if queue_runnable && let Some(next_due_at_ms) = queue_summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if queue_runnable && let Some(next_due_at_ms) = queue_retention.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if cron_runnable && let Some(next_due_at_ms) = cron_summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if workflow_runnable && let Some(next_due_at_ms) = workflow_summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if let Some(retry_at) = workflow_pool.retry_at() {
                deadlines.push(WakeDeadline {
                    at: retry_at,
                    reason: WakeReason::Backoff,
                });
            }
            if let Some(retry_at) = pool.retry_at() {
                deadlines.push(WakeDeadline {
                    at: retry_at,
                    reason: WakeReason::Backoff,
                });
            }
            if let Some(retry_at) = queue_pool.retry_at() {
                deadlines.push(WakeDeadline {
                    at: retry_at,
                    reason: WakeReason::Backoff,
                });
            }
            if let Some(retry_at) = cron_pool.retry_at() {
                deadlines.push(WakeDeadline {
                    at: retry_at,
                    reason: WakeReason::Backoff,
                });
            }
            let wait = self.wake.wait(observed_generation, &deadlines);
            tokio::pin!(wait);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                completed = dispatches.join_next_with_id(), if !dispatches.is_empty() => {
                    if let Some(completed) = completed {
                        self.release_completed(
                            completed_kind(completed, &mut dispatch_kinds)?,
                            &mut admission,
                        );
                    }
                }
                reason = &mut wait => self.observe_wake(reason),
            }
        }

        self.wake.notify();
        version_reconcile.shutdown().await;
        let _ = bounded_drain(
            self.clock.as_ref(),
            Duration::from_millis(self.config.shutdown_drain_ms),
            &mut dispatches,
        )
        .await;
        admission.release(
            SchedulerKind::Alarm,
            admission.pool_in_flight(SchedulerKind::Alarm),
        );
        admission.release(
            SchedulerKind::Queue,
            admission.pool_in_flight(SchedulerKind::Queue),
        );
        admission.release(
            SchedulerKind::Cron,
            admission.pool_in_flight(SchedulerKind::Cron),
        );
        admission.release(
            SchedulerKind::Workflow,
            admission.pool_in_flight(SchedulerKind::Workflow),
        );
        self.store_admission_metrics(&admission);
        Ok(())
    }

    /// Claim and deliver one deterministic due batch without real scheduler sleeps.
    pub async fn poll_once(self: &Arc<Self>) -> Result<usize, PlatformError> {
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let queue = self.config.pool(SchedulerKind::Queue);
        let cron = self.config.pool(SchedulerKind::Cron);
        let workflow = self.config.pool(SchedulerKind::Workflow);
        let mut completed = self.poll_queue_once()?;
        if queue.enabled && !self.is_paused() && !self.queue_paused.load(Ordering::Acquire) {
            let batches = self
                .claim_queue_consumers(
                    queue
                        .claim_batch
                        .min(queue.max_in_flight)
                        .min(self.config.max_in_flight),
                )
                .await?;
            completed = completed.saturating_add(batches.len());
            stream::iter(batches)
                .for_each_concurrent(
                    usize::try_from(queue.max_in_flight).unwrap_or(usize::MAX),
                    |batch| {
                        let service = self.clone();
                        async move { service.dispatch_queue_batch(batch).await }
                    },
                )
                .await;
        }
        if cron.enabled && !self.is_paused() && !self.cron_paused.load(Ordering::Acquire) {
            let runs = self
                .claim_cron(
                    cron.claim_batch
                        .min(cron.max_in_flight)
                        .min(self.config.max_in_flight),
                )
                .await?;
            completed = completed.saturating_add(runs.len());
            stream::iter(runs)
                .for_each_concurrent(
                    usize::try_from(cron.max_in_flight).unwrap_or(usize::MAX),
                    |run| {
                        let service = self.clone();
                        async move { service.dispatch_cron_run(run).await }
                    },
                )
                .await;
        }
        let workflow_summary = self
            .store
            .workflow_workload_summary(self.observed_wall_time_ms())?;
        if workflow.enabled
            && !self.is_paused()
            && !self.workflow_paused.load(Ordering::Acquire)
            && self.workflow_pool_state() != SchedulerPoolState::CircuitOpen
            && (workflow_summary.ready > 0 || workflow_summary.expired > 0)
        {
            let runs = self
                .claim_workflows(
                    workflow
                        .claim_batch
                        .min(workflow.max_in_flight)
                        .min(self.config.max_in_flight),
                )
                .await?;
            completed = completed.saturating_add(runs.len());
            stream::iter(runs)
                .for_each_concurrent(
                    usize::try_from(workflow.max_in_flight).unwrap_or(usize::MAX),
                    |run| {
                        let service = self.clone();
                        async move { service.dispatch_workflow_run(run).await }
                    },
                )
                .await;
        }
        if !alarm.enabled || self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
            return Ok(completed);
        }
        let batch = alarm
            .claim_batch
            .min(alarm.max_in_flight)
            .min(self.config.max_in_flight);
        let jobs = self.claim(batch).await?;
        let count = jobs.len();
        stream::iter(jobs)
            .for_each_concurrent(usize::try_from(batch).unwrap_or(usize::MAX), |job| {
                let service = self.clone();
                async move { service.dispatch_one(job).await }
            })
            .await;
        Ok(completed.saturating_add(count))
    }
}
