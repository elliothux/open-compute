//! Fixed low-cardinality resource and binding metric definitions.

use super::{Inner, write_help};
use std::fmt::Write as _;

/// Resource lifecycle operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ResourceOperation {
    /// Create.
    Create,
    /// Read one resource.
    Get,
    /// List resources.
    List,
    /// Rename.
    Rename,
    /// Delete.
    Delete,
}

impl ResourceOperation {
    const ALL: [Self; 5] = [
        Self::Create,
        Self::Delete,
        Self::Get,
        Self::List,
        Self::Rename,
    ];

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Create => 0,
            Self::Delete => 1,
            Self::Get => 2,
            Self::List => 3,
            Self::Rename => 4,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Get => "get",
            Self::List => "list",
            Self::Rename => "rename",
            Self::Delete => "delete",
        }
    }
}

/// Private binding backend operation label.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BindingBackendOperation {
    /// Read.
    Get,
    /// Write.
    Put,
    /// Delete.
    Delete,
}

impl BindingBackendOperation {
    const ALL: [Self; 3] = [Self::Delete, Self::Get, Self::Put];

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Delete => 0,
            Self::Get => 1,
            Self::Put => 2,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }
}

pub(super) fn write_resource_metrics(out: &mut String, metrics: &Inner) {
    write_help(
        out,
        "resource_operations_total",
        "counter",
        "P0 KV lifecycle operation outcomes",
    );
    for operation in ResourceOperation::ALL {
        let index = operation.index();
        for success in [false, true] {
            writeln!(
                out,
                "resource_operations_total{{kind=\"kv_namespace\",operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                super::success_outcome(success),
                metrics.resource_operations[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "resource_operation_duration_seconds",
        "gauge",
        "Last P0 KV lifecycle operation duration",
    );
    for operation in ResourceOperation::ALL {
        writeln!(
            out,
            "resource_operation_duration_seconds{{kind=\"kv_namespace\",operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.resource_duration[operation.index()]
        )
        .ok();
    }
    write_help(out, "resource_open_handles", "gauge", "Open P0 KV handles");
    writeln!(
        out,
        "resource_open_handles{{kind=\"kv_namespace\"}} {}",
        metrics.resource_open_handles
    )
    .ok();
    write_help(
        out,
        "resource_pin_wait_seconds",
        "gauge",
        "Last P0 KV pin drain wait",
    );
    writeln!(
        out,
        "resource_pin_wait_seconds{{kind=\"kv_namespace\"}} {}",
        metrics.resource_pin_wait
    )
    .ok();
    write_help(
        out,
        "resource_reconcile_total",
        "counter",
        "P0 KV reconcile outcomes",
    );
    for (deleting, state) in [(false, "creating"), (true, "deleting")] {
        for success in [false, true] {
            let index = usize::from(deleting) * 2 + usize::from(success);
            writeln!(
                out,
                "resource_reconcile_total{{kind=\"kv_namespace\",state=\"{state}\",outcome=\"{}\"}} {}",
                super::success_outcome(success),
                metrics.resource_reconcile[index]
            )
            .ok();
        }
    }
    write_help(
        out,
        "binding_backend_requests_total",
        "counter",
        "Private P0 KV binding backend outcomes",
    );
    for operation in BindingBackendOperation::ALL {
        let index = operation.index();
        for success in [false, true] {
            writeln!(
                out,
                "binding_backend_requests_total{{kind=\"kv_namespace\",operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                super::success_outcome(success),
                metrics.binding_backend_requests[index * 2 + usize::from(success)]
            )
            .ok();
        }
    }
    write_help(
        out,
        "binding_backend_bytes_total",
        "counter",
        "Private P0 KV binding backend bytes",
    );
    writeln!(
        out,
        "binding_backend_bytes_total{{kind=\"kv_namespace\",direction=\"ingress\"}} {}",
        metrics.binding_backend_bytes[0]
    )
    .ok();
    writeln!(
        out,
        "binding_backend_bytes_total{{kind=\"kv_namespace\",direction=\"egress\"}} {}",
        metrics.binding_backend_bytes[1]
    )
    .ok();
    write_help(
        out,
        "binding_protocol_errors_total",
        "counter",
        "Malformed private P0 KV binding frames",
    );
    writeln!(
        out,
        "binding_protocol_errors_total{{kind=\"kv_namespace\"}} {}",
        metrics.binding_protocol_errors
    )
    .ok();
}
