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
    const ALL: [Self; 3] = [Self::Connect, Self::Fetch, Self::Rpc];

    const fn index(self) -> usize {
        match self {
            Self::Connect => 0,
            Self::Fetch => 1,
            Self::Rpc => 2,
        }
    }

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
    const ALL: [Self; 3] = [Self::Promotion, Self::Restart, Self::Delete];

    const fn index(self) -> usize {
        match self {
            Self::Promotion => 0,
            Self::Restart => 1,
            Self::Delete => 2,
        }
    }

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
    const ALL: [Self; 2] = [Self::Creating, Self::Deleting];

    const fn index(self) -> usize {
        match self {
            Self::Creating => 0,
            Self::Deleting => 1,
        }
    }

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
    for operation in DoOperation::ALL {
        let index = operation.index();
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_dispatch_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                super::success_outcome(success),
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
    for operation in DoOperation::ALL {
        writeln!(
            out,
            "oc_do_dispatch_duration_seconds{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.do_dispatch_duration[operation.index()]
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
    for reason in DoFacetReloadReason::ALL {
        writeln!(
            out,
            "oc_do_facet_reload_total{{reason=\"{}\"}} {}",
            reason.as_str(),
            metrics.do_facet_reload[reason.index()]
        )
        .ok();
    }
    write_help(
        out,
        "oc_do_object_reconcile_total",
        "counter",
        "Durable Object lifecycle reconciliation outcomes",
    );
    for state in DoReconcileState::ALL {
        let index = state.index();
        for success in [false, true] {
            writeln!(
                out,
                "oc_do_object_reconcile_total{{state=\"{}\",outcome=\"{}\"}} {}",
                state.as_str(),
                super::success_outcome(success),
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
        let index = operation.index();
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
        let index = reason.index();
        guard.do_facet_reload[index] = guard.do_facet_reload[index].saturating_add(1);
    }

    pub(crate) fn inc_do_reconcile(&self, state: DoReconcileState, success: bool) {
        let mut guard = self.lock();
        let index = state.index() * 2 + usize::from(success);
        guard.do_reconcile[index] = guard.do_reconcile[index].saturating_add(1);
    }

    pub(crate) fn set_do_storage_watermark(&self, watermark: usize) {
        self.lock().do_storage_watermark = watermark.min(2);
    }
}
