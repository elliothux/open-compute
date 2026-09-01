//! Fixed low-cardinality P0.7 Durable Object metrics.

use super::{Inner, write_help};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DoOperation {
    Connect,
    Fetch,
    Rpc,
}

impl DoOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Fetch => "fetch",
            Self::Rpc => "rpc",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DoFacetReloadReason {
    Promotion,
    Restart,
    Delete,
}

impl DoFacetReloadReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Promotion => "promotion",
            Self::Restart => "restart",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DoReconcileState {
    Creating,
    Deleting,
}

impl DoReconcileState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Deleting => "deleting",
        }
    }
}

pub(super) fn write_do_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "oc_do_dispatch_total",
        "counter",
        "Durable Object dispatch admission outcomes",
    );
    for operation in operations() {
        let index = operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_dispatch_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
                metrics.do_dispatch[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "oc_do_dispatch_duration_seconds",
        "gauge",
        "Last Durable Object dispatch admission duration",
    );
    for operation in operations() {
        writeln!(
            out,
            "oc_do_dispatch_duration_seconds{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.do_dispatch_duration[operation_index(operation)]
        )
        .ok();
    }
    write_help(
        out,
        "oc_do_active_host_actors",
        "gauge",
        "Registered live Durable Object host actors",
    );
    writeln!(out, "oc_do_active_host_actors {}", metrics.do_active_hosts).ok();
    write_help(
        out,
        "oc_do_facet_reload_total",
        "counter",
        "Durable Object facet reload causes",
    );
    for reason in reload_reasons() {
        writeln!(
            out,
            "oc_do_facet_reload_total{{reason=\"{}\"}} {}",
            reason.as_str(),
            metrics.do_facet_reload[reload_index(reason)]
        )
        .ok();
    }
    write_help(
        out,
        "oc_do_object_reconcile_total",
        "counter",
        "Durable Object lifecycle reconciliation outcomes",
    );
    for state in reconcile_states() {
        let index = reconcile_index(state);
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_object_reconcile_total{{state=\"{}\",outcome=\"{}\"}} {}",
                state.as_str(),
                outcome(success),
                metrics.do_reconcile[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "oc_do_storage_watermark",
        "gauge",
        "Durable Object localDisk watermark state",
    );
    for (index, state) in ["normal", "high", "stop"].into_iter().enumerate() {
        writeln!(
            out,
            "oc_do_storage_watermark{{state=\"{state}\"}} {}",
            u64::from(metrics.do_storage_watermark == index)
        )
        .ok();
    }
}

impl super::MetricsRegistry {
    pub(crate) fn observe_do_dispatch(
        &self,
        operation: DoOperation,
        success: bool,
        duration: Duration,
    ) {
        let index = operation_index(operation);
        let mut guard = self.lock();
        guard.do_dispatch[index * 2 + usize::from(success)] =
            guard.do_dispatch[index * 2 + usize::from(success)].saturating_add(1);
        guard.do_dispatch_duration[index] = duration.as_secs_f64();
    }

    pub(crate) fn set_do_active_hosts(&self, hosts: u64) {
        self.lock().do_active_hosts = hosts;
    }

    pub(crate) fn inc_do_facet_reload(&self, reason: DoFacetReloadReason) {
        let mut guard = self.lock();
        let index = reload_index(reason);
        guard.do_facet_reload[index] = guard.do_facet_reload[index].saturating_add(1);
    }

    pub(crate) fn inc_do_reconcile(&self, state: DoReconcileState, success: bool) {
        let mut guard = self.lock();
        let index = reconcile_index(state) * 2 + usize::from(success);
        guard.do_reconcile[index] = guard.do_reconcile[index].saturating_add(1);
    }

    pub(crate) fn set_do_storage_watermark(&self, watermark: usize) {
        self.lock().do_storage_watermark = watermark.min(2);
    }
}

fn operations() -> [DoOperation; 3] {
    [DoOperation::Connect, DoOperation::Fetch, DoOperation::Rpc]
}

fn operation_index(operation: DoOperation) -> usize {
    operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

fn reload_reasons() -> [DoFacetReloadReason; 3] {
    [
        DoFacetReloadReason::Promotion,
        DoFacetReloadReason::Restart,
        DoFacetReloadReason::Delete,
    ]
}

fn reload_index(reason: DoFacetReloadReason) -> usize {
    reload_reasons()
        .iter()
        .position(|candidate| *candidate == reason)
        .unwrap()
}

fn reconcile_states() -> [DoReconcileState; 2] {
    [DoReconcileState::Creating, DoReconcileState::Deleting]
}

fn reconcile_index(state: DoReconcileState) -> usize {
    reconcile_states()
        .iter()
        .position(|candidate| *candidate == state)
        .unwrap()
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}
