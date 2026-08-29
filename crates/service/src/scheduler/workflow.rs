//! Frozen Workflow activation, private lease heartbeats, and terminal release.

use super::*;
use crate::metrics::SchedulerClaimOutcome;
use crate::runtime_bridge::{WorkflowOutcome, WorkflowRunRequest};
use open_compute_storage::WorkflowRepository;
use open_compute_storage::scheduler::{ClaimedWorkflowRun, WorkflowCompletion, WorkflowState};
use open_compute_workers::WorkflowController;

impl SchedulerService {
    /// Resume a bounded validation page; transport Unknown leaves its durable target unchanged.
    pub async fn reconcile_workflow_versions(&self, limit: u32) -> Result<(), PlatformError> {
        // Runtime probes are deferred while startup or quarantine prevents
        // dispatch. Durable catalog/saga reconciliation remains available; no
        // validating version becomes ready and no infrastructure failure is
        // charged for a probe that could not start.
        if self.transport.ensure_workflow_admission().is_err() {
            return Ok(());
        }
        let versions = {
            let mut cursor = self
                .workflow_version_cursor
                .lock()
                .map_err(|_| scheduler_task_failed())?;
            let versions = WorkflowRepository::new(self.storage.db())
                .pending_versions(*cursor, limit.min(32))?;
            *cursor = versions.last().map(|version| version.target.version_id);
            versions
        };
        for version in versions {
            crate::workflow_http::validate_version(self.storage.clone(), &self.transport, version)
                .await?;
        }
        // A separate cursor also checks old versions pinned by live instances. The normal
        // RuntimeSource path revalidates immutable artifact bytes even with a warm loader.
        let instances = {
            let mut cursor = self
                .workflow_artifact_cursor
                .lock()
                .map_err(|_| scheduler_task_failed())?;
            let ids = self.store.workflow_instance_ids(*cursor, limit.min(32))?;
            *cursor = ids.last().copied();
            ids
        };
        for id in instances {
            let record = self
                .store
                .workflow_instance(id)?
                .ok_or_else(scheduler_task_failed)?;
            if record.state.is_terminal() {
                continue;
            }
            let frozen = &record.identity.target;
            let probe = self.transport.probe_workflow(frozen).await;
            if let Err(error) = probe {
                if matches!(
                    error.code(),
                    ErrorCode::ArtifactIntegrityError
                        | ErrorCode::WorkflowInvariantViolation
                        | ErrorCode::WorkflowVersionNotReady
                ) {
                    WorkflowRepository::new(self.storage.db()).mark_unavailable(
                        frozen.account_id,
                        frozen.definition_id,
                        self.observed_wall_time_ms(),
                    )?;
                    self.set_workflow_pool_state(SchedulerPoolState::CircuitOpen);
                    self.observe_pool_health(SchedulerPoolState::CircuitOpen);
                    return Err(error);
                }
                self.workflow_unknown();
            }
        }
        Ok(())
    }

    pub(crate) async fn claim_workflows(
        &self,
        batch: u32,
    ) -> Result<Vec<ClaimedWorkflowRun>, PlatformError> {
        let storage = self.storage.clone();
        let store = self.store.clone();
        let config = self.workflows.clone();
        let transport = self.transport.clone();
        let cursor = self.workflow_claim_cursor.clone();
        let now_ms = self.observed_wall_time_ms();
        let started = self.clock.monotonic_now();
        let result = tokio::task::spawn_blocking(move || {
            transport.ensure_workflow_admission()?;
            let mut cursor = cursor.lock().map_err(|_| scheduler_task_failed())?;
            let controller = WorkflowController::new(&storage, &store, &config);
            let mut runs = Vec::new();
            for _ in 0..batch.min(32) {
                // A quarantined invocation may still own runtime resources. Do not consume
                // fresh leases until the supervised child has rotated its generation.
                if !runs.is_empty() && transport.ensure_workflow_admission().is_err() {
                    break;
                }
                let Some(run) = controller.claim(now_ms, &mut cursor)? else {
                    break;
                };
                runs.push(run);
            }
            Ok(runs)
        })
        .await
        .map_err(|_| scheduler_task_failed())?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_scheduler_claim_duration(
                SchedulerKind::Workflow,
                self.clock
                    .monotonic_now()
                    .saturating_duration_since(started),
            );
            metrics.inc_scheduler_claim(
                SchedulerKind::Workflow,
                match &result {
                    Ok(runs) if runs.is_empty() => SchedulerClaimOutcome::Empty,
                    Ok(_) => SchedulerClaimOutcome::Claimed,
                    Err(_) => SchedulerClaimOutcome::Error,
                },
            );
            if let Ok(runs) = &result {
                metrics.inc_scheduler_claim_expired(
                    SchedulerKind::Workflow,
                    runs.iter().filter(|run| run.recovered).count() as u64,
                );
            }
        }
        result
    }

    /// Reconcile a bounded Workflow page without guessing execution history or target identity.
    pub fn repair_workflows(&self, limit: u32) -> Result<(), PlatformError> {
        let mut cursor = self
            .workflow_reconcile_cursor
            .lock()
            .map_err(|_| scheduler_task_failed())?;
        let result = WorkflowController::new(&self.storage, &self.store, &self.workflows)
            .reconcile(&mut cursor, limit.min(1000), self.observed_wall_time_ms());
        if let Some(metrics) = &self.metrics {
            metrics.workflow_reconcile(result.is_ok());
            if let Ok(operations) = WorkflowRepository::new(self.storage.db()).inspect_operations()
            {
                metrics.workflow_operations(&operations, self.observed_wall_time_ms());
            }
            if let (Ok(summary), Ok(workload)) = (
                self.store.inspect_workflows(self.observed_wall_time_ms()),
                self.store
                    .workflow_workload_summary(self.observed_wall_time_ms()),
            ) {
                metrics.workflow_summary(
                    &summary,
                    workload.oldest_due_at_ms.map_or(0.0, |due| {
                        self.observed_wall_time_ms().saturating_sub(due).max(0) as f64 / 1000.0
                    }),
                );
            }
        }
        result
    }

    pub(crate) async fn dispatch_workflow_run(self: Arc<Self>, run: ClaimedWorkflowRun) {
        let mut observation = self.metrics.as_ref().map(MetricsRegistry::workflow_run);
        let request = WorkflowRunRequest {
            fence: run.fence.clone(),
            external_instance_id: run.external_instance_id.clone(),
            definition_name: run.target.definition_name.clone(),
            created_at_ms: run.created_at_ms,
            payload_json: run.input_json.clone(),
        };
        let version = run.target.clone();
        let dispatch = self.transport.dispatch_workflow(
            &version,
            &request,
            Duration::from_millis(self.workflows.dispatch_timeout_ms),
        );
        tokio::pin!(dispatch);
        let mut heartbeat_live = true;
        let response = loop {
            let deadline = self
                .clock
                .monotonic_deadline(Duration::from_millis(self.workflows.heartbeat_ms));
            tokio::select! {
                response=&mut dispatch=>break response,
                ()=self.clock.sleep_until(deadline),if heartbeat_live=>{
                    let store=self.store.clone();let fence=run.fence.clone();let config=self.workflows.clone();let now_ms=self.observed_wall_time_ms();
                    let result=tokio::task::spawn_blocking(move||store.heartbeat_workflow(&fence,now_ms,&config)).await;
                    // Yield may clear the lease before its HTTP acknowledgement arrives. Keep the
                    // permit until the bounded transport actually ends; never infer RPC cancellation.
                    heartbeat_live=matches!(result,Ok(Ok(())));
                }
            }
        };
        let Ok(response) = response else {
            self.workflow_unknown();
            return;
        };
        let store = self.store.clone();
        let storage = self.storage.clone();
        let config = self.workflows.clone();
        let transport = self.transport.clone();
        let now_ms = self.observed_wall_time_ms();
        let result = tokio::task::spawn_blocking(move || {
            transport.commit_workflow(response, |outcome| {
                if matches!(outcome, WorkflowOutcome::Unknown { .. }) {
                    return Err(scheduler_task_failed());
                }
                let record = store
                    .workflow_instance(run.fence.instance_id)?
                    .ok_or_else(scheduler_task_failed)?;
                if record.identity.instance_generation != run.fence.instance_generation
                    || record.identity.target != run.target
                {
                    return Err(PlatformError::new(
                        ErrorCode::WorkflowRunStale,
                        "Workflow generation is stale",
                    ));
                }
                let completion = match outcome {
                    WorkflowOutcome::Suspended { .. } => {
                        if record.run_token.as_ref() == Some(&run.fence.run_token) {
                            return Err(scheduler_task_failed());
                        }
                        return Ok(None);
                    }
                    WorkflowOutcome::Complete {
                        output_json,
                        final_ordinal,
                    } => WorkflowCompletion::Complete {
                        output_json,
                        final_ordinal,
                    },
                    WorkflowOutcome::Errored { error_code, .. } => WorkflowCompletion::Errored {
                        code: open_compute_core::workflow::terminal_error_code(&error_code)?,
                    },
                    WorkflowOutcome::Unknown { .. } => return Err(scheduler_task_failed()),
                };
                let state = store.finish_workflow(&run.fence, &completion, now_ms, &config)?;
                if !state.is_terminal() {
                    return Ok(None);
                }
                WorkflowRepository::new(storage.db()).retain_instance(&record.identity, now_ms)?;
                Ok(Some(state))
            })
        })
        .await;
        match result {
            Ok(Ok(state)) => {
                if let Some(observation) = &mut observation {
                    match state {
                        Some(WorkflowState::Complete) => {
                            observation.finish(crate::metrics::WorkflowOutcome::Success);
                        }
                        Some(_) => observation.finish(crate::metrics::WorkflowOutcome::Error),
                        None => observation.suspended(),
                    }
                }
                self.workflow_infra_failures.store(0, Ordering::Release);
                self.wake.notify();
            }
            Ok(Err(error))
                if matches!(
                    error.code(),
                    ErrorCode::WorkflowRunStale | ErrorCode::WorkflowStepStale
                ) =>
            {
                if let Some(metrics) = &self.metrics {
                    metrics.workflow_stale(error.code() == ErrorCode::WorkflowStepStale);
                    metrics.inc_scheduler_stale_completion(SchedulerKind::Workflow);
                }
            }
            _ => self.workflow_unknown(),
        }
    }

    fn workflow_unknown(&self) {
        // Repeated unknown activations require operator attention; leases and history remain intact.
        if self.workflow_infra_failures.fetch_add(1, Ordering::AcqRel) >= 2 {
            self.set_workflow_pool_state(SchedulerPoolState::CircuitOpen);
            self.observe_pool_health(SchedulerPoolState::CircuitOpen);
        }
        tracing::warn!(
            code = "WORKFLOW_RUNTIME_UNAVAILABLE",
            "Workflow result unknown; lease retained"
        );
        self.wake.notify();
    }

    pub(super) fn workflow_pool_state(&self) -> SchedulerPoolState {
        decode_pool_state(self.workflow_pool_state.load(Ordering::Acquire))
    }

    pub(super) fn set_workflow_pool_state(&self, state: SchedulerPoolState) {
        self.workflow_pool_state
            .store(encode_pool_state(state), Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_scheduler_pool_state(SchedulerKind::Workflow, state);
        }
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
