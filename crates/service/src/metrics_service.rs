//! Fixed low-cardinality Service Binding metric definitions.

use super::{Inner, write_help};
use std::fmt::Write as _;

/// Service invocation category visible to bounded platform metrics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceMetricOperation {
    /// Default fetch, including static-asset routing.
    DefaultFetch,
    /// Fetch through a named Worker entrypoint.
    NamedFetch,
    /// Default or named Worker RPC.
    Rpc,
    /// Method/getter call on a retained native capability.
    Capability,
}

impl ServiceMetricOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DefaultFetch => "default_fetch",
            Self::NamedFetch => "named_fetch",
            Self::Rpc => "rpc",
            Self::Capability => "capability",
        }
    }
}

pub(super) fn write_service_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "service_invocations_total",
        "counter",
        "Authenticated Service invocation outcomes",
    );
    for operation in service_operations() {
        let index = service_operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "service_invocations_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                if success { "success" } else { "error" },
                metrics.service_invocations[index * 2 + usize::from(success)],
            )
            .ok();
        }
    }
    write_help(
        out,
        "service_invocation_duration_seconds",
        "gauge",
        "Last authenticated Service invocation duration",
    );
    for operation in service_operations() {
        writeln!(
            out,
            "service_invocation_duration_seconds{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.service_invocation_duration[service_operation_index(operation)],
        )
        .ok();
    }
    write_help(
        out,
        "service_invocation_roots",
        "gauge",
        "Live Service invocation roots",
    );
    writeln!(out, "service_invocation_roots {}", metrics.service_roots).ok();
    write_help(
        out,
        "service_invocation_operations",
        "gauge",
        "Live Service invocation operations",
    );
    writeln!(
        out,
        "service_invocation_operations {}",
        metrics.service_operations,
    )
    .ok();
    write_help(
        out,
        "service_capability_retentions",
        "gauge",
        "Live Service native capability retention groups",
    );
    writeln!(
        out,
        "service_capability_retentions {}",
        metrics.service_retentions,
    )
    .ok();
}

pub(super) fn service_operation_index(operation: ServiceMetricOperation) -> usize {
    service_operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

const fn service_operations() -> [ServiceMetricOperation; 4] {
    [
        ServiceMetricOperation::DefaultFetch,
        ServiceMetricOperation::NamedFetch,
        ServiceMetricOperation::Rpc,
        ServiceMetricOperation::Capability,
    ]
}
