//! production storage APIs, without SQL-authored step outcomes.

use super::*;
use open_compute_core::WorkflowOperationId;

const NULL_VALUE: &str = "T0NEVgECAA==";
const TRUE_VALUE: &str = "T0NEVgECAw==";
const ONE_VALUE: &str = "T0NEVgECBD/wAAAAAAAA";
const TWO_VALUE: &str = "T0NEVgECBEAAAAAAAAAA";
const SEVEN_VALUE: &str = "T0NEVgECBEAcAAAAAAAA";
const EIGHT_VALUE: &str = "T0NEVgECBEAgAAAAAAAA";
const FORTY_TWO_VALUE: &str = "T0NEVgECBEBFAAAAAAAA";
const ARRAY_VALUE: &str = "T0NEVgECEAAAAAIAAAAABD/wAAAAAAAABEAAAAAAAAAA";
const DECISION_VALUE: &str = "T0NEVgECEQAAAAEAAAAIZGVjaXNpb24D";

fn do_step(ordinal: u32, config: Value) -> WorkflowStepDescriptor {
    descriptor(ordinal, WorkflowStepKind::Do, config)
}
fn attempt(ordinal: u32, grant: WorkflowStepGrant) -> WorkflowStepAttempt {
    let WorkflowStepGrant::Run {
        step_token,
        attempt,
        ..
    } = grant
    else {
        panic!("expected callback grant")
    };
    WorkflowStepAttempt {
        ordinal,
        attempt,
        step_token,
    }
}
fn claim(
    store: &SchedulerStore,
    fence: &WorkflowFence,
    step: &WorkflowStepDescriptor,
    now: i64,
    limits: &WorkflowsConfig,
) -> WorkflowStepAttempt {
    attempt(
        step.ordinal,
        store
            .claim_workflow_batch(
                fence,
                std::slice::from_ref(step),
                limits.dispatch_timeout_ms,
                now,
                limits,
            )
            .unwrap()
            .remove(0),
    )
}

fn wait_result(
    store: &SchedulerStore,
    fence: &WorkflowFence,
    step: &WorkflowStepDescriptor,
    now: i64,
    limits: &WorkflowsConfig,
) -> WorkflowStepResult {
    store
        .claim_workflow_batch(
            fence,
            std::slice::from_ref(step),
            limits.dispatch_timeout_ms,
            now,
            limits,
        )
        .unwrap();
    store
        .workflow_step_result(fence, step.ordinal, now)
        .unwrap()
}

mod current_pause_drains_grants_and_replays_completed_steps_without_extending_deadlines;

mod current_pause_survives_expired_run_recovery_and_terminate_rejects_late_commits;

mod current_production_do_sleep_event_resume_and_terminal_are_durable;

mod current_retry_is_claimed_only_when_due_and_settled_failures_can_be_caught;

mod current_unknown_recovery_does_not_extend_attempt_and_late_success_loses_to_deadline;

mod current_batch_commits_independently_then_yields_after_siblings_drain;

mod mixed_graph_group_persists_explicit_dependencies_and_replays_all_step_kinds;

mod current_buffering_timeout_boundary_and_budget_yield_do_not_consume_callbacks;

mod rollback_replays_completed_handlers_recovers_inflight_work_and_terminates;

mod dynamic_retry_delay_is_durable_and_resolved_under_the_exact_attempt;

mod rejected_dynamic_delay_and_batch_shapes_fail_closed_before_new_work;
