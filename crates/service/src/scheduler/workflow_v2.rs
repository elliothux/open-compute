//! Production V2 dispatch keeps suspension distinct from terminal retention and Unknown recovery.

use super::*;
use crate::runtime_bridge::WorkflowV2Outcome;
use open_compute_storage::scheduler::WorkflowState;

impl SchedulerService {
    pub(super) async fn dispatch_workflow_run_v2(self: Arc<Self>, run: ClaimedWorkflowRun) {
        let mut observation = self.metrics.as_ref().map(MetricsRegistry::workflow_run);
        let request = WorkflowRunRequest {
            fence: run.fence.clone(),
            external_instance_id: run.external_instance_id.clone(),
            definition_name: run.target.definition_name.clone(),
            created_at_ms: run.created_at_ms,
            payload_json: run.input_json.clone(),
        };
        let version = run.target.clone();
        let dispatch = self.transport.dispatch_workflow_v2(
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
            transport.commit_workflow_v2(response, |outcome| {
                if matches!(outcome, WorkflowV2Outcome::Unknown { .. }) {
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
                    WorkflowV2Outcome::Suspended { .. } => {
                        if record.run_token.as_ref() == Some(&run.fence.run_token) {
                            return Err(scheduler_task_failed());
                        }
                        return Ok(None);
                    }
                    WorkflowV2Outcome::Complete {
                        output_json,
                        final_ordinal,
                    } => WorkflowCompletion::Complete {
                        output_json,
                        final_ordinal,
                    },
                    WorkflowV2Outcome::Errored { error_code, .. } => WorkflowCompletion::Errored {
                        code: open_compute_core::workflow::terminal_error_code_v2(&error_code)?,
                    },
                    WorkflowV2Outcome::Unknown { .. } => return Err(scheduler_task_failed()),
                };
                let state = store.finish_workflow_v2(&run.fence, &completion, now_ms, &config)?;
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
}
