//! Fixed low-cardinality P0.8 scheduler and Durable Object alarm metrics.

use super::{Inner, write_help};
use open_compute_core::{SchedulerKind, SchedulerPoolState, WorkloadSummary};
use open_compute_storage::SchedulerSummary;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SchedulerClaimOutcome {
    Claimed,
    Empty,
    Error,
}

impl SchedulerClaimOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Empty => "empty",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlarmOutcome {
    Success,
    Stale,
    NotDue,
    Retry,
    Exhausted,
    Error,
}

impl AlarmOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Stale => "stale",
            Self::NotDue => "not_due",
            Self::Retry => "retry",
            Self::Exhausted => "exhausted",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlarmMutation {
    Set,
    Delete,
    Clear,
}

impl AlarmMutation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Delete => "delete",
            Self::Clear => "clear",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlarmRepairSource {
    Read,
    Activation,
    Scan,
}

impl AlarmRepairSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Activation => "activation",
            Self::Scan => "scan",
        }
    }
}

pub(super) fn write_scheduler_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "oc_scheduler_jobs",
        "gauge",
        "Scheduler jobs by kind and state",
    );
    for (index, state) in ["scheduled", "claimed", "discarding"]
        .into_iter()
        .enumerate()
    {
        writeln!(
            out,
            "oc_scheduler_jobs{{kind=\"do_alarm\",state=\"{state}\"}} {}",
            metrics.scheduler_jobs[index]
        )
        .ok();
    }
    write_help(
        out,
        "oc_scheduler_claim_total",
        "counter",
        "Scheduler claim pass outcomes",
    );
    for outcome in claim_outcomes() {
        writeln!(
            out,
            "oc_scheduler_claim_total{{outcome=\"{}\"}} {}",
            outcome.as_str(),
            metrics.scheduler_claim[claim_index(outcome)]
        )
        .ok();
    }
    write_help(
        out,
        "oc_scheduler_dispatch_duration_seconds",
        "gauge",
        "Last scheduler dispatch duration",
    );
    for outcome in alarm_outcomes() {
        writeln!(
            out,
            "oc_scheduler_dispatch_duration_seconds{{kind=\"do_alarm\",outcome=\"{}\"}} {}",
            outcome.as_str(),
            metrics.scheduler_dispatch_duration[alarm_index(outcome)]
        )
        .ok();
    }
    write_help(
        out,
        "oc_scheduler_claim_expired_total",
        "counter",
        "Expired alarm claims recovered",
    );
    writeln!(
        out,
        "oc_scheduler_claim_expired_total{{kind=\"do_alarm\"}} {}",
        metrics.scheduler_claim_expired
    )
    .ok();
    write_help(
        out,
        "oc_scheduler_in_flight",
        "gauge",
        "Alarm dispatches currently in flight",
    );
    writeln!(
        out,
        "oc_scheduler_in_flight{{kind=\"do_alarm\"}} {}",
        metrics.scheduler_in_flight
    )
    .ok();
    write_p2_scheduler_metrics(out, metrics);
    write_help(
        out,
        "oc_do_alarm_mutation_total",
        "counter",
        "Alarm authority mutation outcomes",
    );
    for operation in mutations() {
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_alarm_mutation_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.alarm_mutation[mutation_index(operation) * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "oc_do_alarm_delivery_total",
        "counter",
        "Alarm delivery outcomes and retry buckets",
    );
    for delivery in alarm_outcomes() {
        for retry in 0..=6 {
            writeln!(
                out,
                "oc_do_alarm_delivery_total{{outcome=\"{}\",retry_bucket=\"{retry}\"}} {}",
                delivery.as_str(),
                metrics.alarm_delivery[alarm_index(delivery) * 7 + retry]
            )
            .ok();
        }
    }
    write_help(
        out,
        "oc_do_alarm_repair_total",
        "counter",
        "Alarm projection repair outcomes",
    );
    for source in repair_sources() {
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_alarm_repair_total{{source=\"{}\",outcome=\"{}\"}} {}",
                source.as_str(),
                outcome(success),
                metrics.alarm_repair[repair_index(source) * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "oc_do_alarm_lag_seconds",
        "gauge",
        "Oldest due alarm lag",
    );
    writeln!(out, "oc_do_alarm_lag_seconds {}", metrics.alarm_lag_seconds).ok();
}

impl super::MetricsRegistry {
    pub(crate) fn observe_scheduler_summary(&self, summary: SchedulerSummary, now_ms: i64) {
        let mut guard = self.lock();
        guard.scheduler_jobs = [summary.scheduled, summary.claimed, summary.discarding];
        guard.alarm_lag_seconds = summary
            .oldest_due_at_ms
            .map_or(0.0, |due| now_ms.saturating_sub(due).max(0) as f64 / 1000.0);
    }

    pub(crate) fn inc_scheduler_claim(&self, outcome: SchedulerClaimOutcome) {
        let mut guard = self.lock();
        let index = claim_index(outcome);
        guard.scheduler_claim[index] = guard.scheduler_claim[index].saturating_add(1);
    }

    pub(crate) fn observe_scheduler_claim_duration(&self, duration: Duration) {
        self.lock().scheduler_claim_duration = duration.as_secs_f64();
    }

    pub(crate) fn observe_scheduler_workload(
        &self,
        kind: SchedulerKind,
        summary: WorkloadSummary,
        now_ms: i64,
    ) {
        let mut guard = self.lock();
        guard.scheduler_ready[kind.index()] = summary.ready;
        if kind == SchedulerKind::Alarm {
            guard.alarm_lag_seconds = summary
                .oldest_due_at_ms
                .map_or(0.0, |due| now_ms.saturating_sub(due).max(0) as f64 / 1000.0);
        }
    }

    pub(crate) fn inc_scheduler_stale_completion(&self, kind: SchedulerKind) {
        let mut guard = self.lock();
        guard.scheduler_stale_completion[kind.index()] =
            guard.scheduler_stale_completion[kind.index()].saturating_add(1);
    }

    pub(crate) fn set_scheduler_pool_state(&self, kind: SchedulerKind, state: SchedulerPoolState) {
        self.lock().scheduler_pool_state[kind.index()] = pool_state_index(state) as u8;
    }

    pub(crate) fn inc_scheduler_wake(&self, reason: &str) {
        let Some(index) = wake_reasons()
            .iter()
            .position(|candidate| *candidate == reason)
        else {
            return;
        };
        let mut guard = self.lock();
        guard.scheduler_wake[index] = guard.scheduler_wake[index].saturating_add(1);
    }

    pub(crate) fn inc_scheduler_claim_expired(&self, count: u64) {
        let mut guard = self.lock();
        guard.scheduler_claim_expired = guard.scheduler_claim_expired.saturating_add(count);
    }

    pub(crate) fn adjust_scheduler_in_flight(&self, starting: bool) {
        let mut guard = self.lock();
        guard.scheduler_in_flight = if starting {
            guard.scheduler_in_flight.saturating_add(1)
        } else {
            guard.scheduler_in_flight.saturating_sub(1)
        };
    }

    pub(crate) fn observe_alarm_delivery(
        &self,
        outcome: AlarmOutcome,
        retry_count: u8,
        duration: Duration,
    ) {
        let mut guard = self.lock();
        let index = alarm_index(outcome);
        guard.scheduler_dispatch_duration[index] = duration.as_secs_f64();
        guard.scheduler_dispatch_last = duration.as_secs_f64();
        let delivery = index * 7 + usize::from(retry_count.min(6));
        guard.alarm_delivery[delivery] = guard.alarm_delivery[delivery].saturating_add(1);
    }

    pub(crate) fn inc_alarm_mutation(&self, operation: AlarmMutation, success: bool) {
        let mut guard = self.lock();
        let index = mutation_index(operation) * 2 + usize::from(success);
        guard.alarm_mutation[index] = guard.alarm_mutation[index].saturating_add(1);
    }

    pub(crate) fn inc_alarm_repair(&self, source: AlarmRepairSource, success: bool) {
        let mut guard = self.lock();
        let index = repair_index(source) * 2 + usize::from(success);
        guard.alarm_repair[index] = guard.alarm_repair[index].saturating_add(1);
    }
}

fn write_p2_scheduler_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "open_compute_scheduler_ready",
        "gauge",
        "Ready scheduler claims by registered workload",
    );
    writeln!(
        out,
        "open_compute_scheduler_ready{{kind=\"do_alarm\"}} {}",
        metrics.scheduler_ready[SchedulerKind::Alarm.index()]
    )
    .ok();
    write_help(
        out,
        "open_compute_scheduler_in_flight",
        "gauge",
        "In-flight scheduler dispatches by registered workload",
    );
    writeln!(
        out,
        "open_compute_scheduler_in_flight{{kind=\"do_alarm\"}} {}",
        metrics.scheduler_in_flight
    )
    .ok();
    write_help(
        out,
        "open_compute_scheduler_claim_total",
        "counter",
        "Scheduler claims by registered workload and fixed outcome",
    );
    for outcome in claim_outcomes() {
        writeln!(
            out,
            "open_compute_scheduler_claim_total{{kind=\"do_alarm\",outcome=\"{}\"}} {}",
            outcome.as_str(),
            metrics.scheduler_claim[claim_index(outcome)]
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_scheduler_dispatch_total",
        "counter",
        "Scheduler dispatches by registered workload and fixed outcome",
    );
    for outcome in alarm_outcomes() {
        let index = alarm_index(outcome);
        let total = metrics.alarm_delivery[index * 7..index * 7 + 7]
            .iter()
            .copied()
            .sum::<u64>();
        writeln!(
            out,
            "open_compute_scheduler_dispatch_total{{kind=\"do_alarm\",outcome=\"{}\"}} {total}",
            outcome.as_str()
        )
        .ok();
    }
    for (name, help, value) in [
        (
            "open_compute_scheduler_claim_latency_seconds",
            "Last scheduler claim latency",
            metrics.scheduler_claim_duration,
        ),
        (
            "open_compute_scheduler_dispatch_latency_seconds",
            "Last scheduler dispatch latency",
            metrics.scheduler_dispatch_last,
        ),
        (
            "open_compute_scheduler_oldest_due_age_seconds",
            "Oldest ready scheduler claim age",
            metrics.alarm_lag_seconds,
        ),
    ] {
        write_help(out, name, "gauge", help);
        writeln!(out, "{name}{{kind=\"do_alarm\"}} {value}").ok();
    }
    for (name, help, value) in [
        (
            "open_compute_scheduler_stale_completion_total",
            "Token-fenced stale completions",
            metrics.scheduler_stale_completion[SchedulerKind::Alarm.index()],
        ),
        (
            "open_compute_scheduler_lease_recovery_total",
            "Expired scheduler claims recovered",
            metrics.scheduler_claim_expired,
        ),
    ] {
        write_help(out, name, "counter", help);
        writeln!(out, "{name}{{kind=\"do_alarm\"}} {value}").ok();
    }
    write_help(
        out,
        "open_compute_scheduler_pool_state",
        "gauge",
        "Registered scheduler pool state",
    );
    let current = usize::from(metrics.scheduler_pool_state[SchedulerKind::Alarm.index()]);
    for (index, state) in pool_states().into_iter().enumerate() {
        let value = usize::from(index == current);
        writeln!(
            out,
            "open_compute_scheduler_pool_state{{kind=\"do_alarm\",state=\"{state}\"}} {value}"
        )
        .ok();
    }
    write_help(
        out,
        "open_compute_scheduler_wake_total",
        "counter",
        "Scheduler wakeups by fixed reason",
    );
    for (index, reason) in wake_reasons().into_iter().enumerate() {
        writeln!(
            out,
            "open_compute_scheduler_wake_total{{reason=\"{reason}\"}} {}",
            metrics.scheduler_wake[index]
        )
        .ok();
    }
}

fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}

fn claim_outcomes() -> [SchedulerClaimOutcome; 3] {
    [
        SchedulerClaimOutcome::Claimed,
        SchedulerClaimOutcome::Empty,
        SchedulerClaimOutcome::Error,
    ]
}

fn alarm_outcomes() -> [AlarmOutcome; 6] {
    [
        AlarmOutcome::Success,
        AlarmOutcome::Stale,
        AlarmOutcome::NotDue,
        AlarmOutcome::Retry,
        AlarmOutcome::Exhausted,
        AlarmOutcome::Error,
    ]
}

fn mutations() -> [AlarmMutation; 3] {
    [
        AlarmMutation::Set,
        AlarmMutation::Delete,
        AlarmMutation::Clear,
    ]
}

fn repair_sources() -> [AlarmRepairSource; 3] {
    [
        AlarmRepairSource::Read,
        AlarmRepairSource::Activation,
        AlarmRepairSource::Scan,
    ]
}

fn claim_index(value: SchedulerClaimOutcome) -> usize {
    claim_outcomes()
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap()
}

fn alarm_index(value: AlarmOutcome) -> usize {
    alarm_outcomes()
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap()
}

fn mutation_index(value: AlarmMutation) -> usize {
    mutations()
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap()
}

fn repair_index(value: AlarmRepairSource) -> usize {
    repair_sources()
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap()
}

fn pool_states() -> [&'static str; 5] {
    ["ready", "paused", "backoff", "circuit_open", "disabled"]
}

fn pool_state_index(value: SchedulerPoolState) -> usize {
    match value {
        SchedulerPoolState::Ready => 0,
        SchedulerPoolState::Paused => 1,
        SchedulerPoolState::Backoff => 2,
        SchedulerPoolState::CircuitOpen => 3,
        SchedulerPoolState::Disabled => 4,
    }
}

fn wake_reasons() -> [&'static str; 5] {
    ["notification", "due", "repair", "backoff", "safety"]
}
