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
    for operation in resource_operations() {
        let index = resource_operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "resource_operations_total{{kind=\"kv_namespace\",operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
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
    for operation in resource_operations() {
        writeln!(
            out,
            "resource_operation_duration_seconds{{kind=\"kv_namespace\",operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.resource_duration[resource_operation_index(operation)]
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
                outcome(success),
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
    for operation in binding_operations() {
        let index = binding_operation_index(operation);
        for success in [false, true] {
            writeln!(
                out,
                "binding_backend_requests_total{{kind=\"kv_namespace\",operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome(success),
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

fn resource_operations() -> [ResourceOperation; 5] {
    [
        ResourceOperation::Create,
        ResourceOperation::Delete,
        ResourceOperation::Get,
        ResourceOperation::List,
        ResourceOperation::Rename,
    ]
}

fn binding_operations() -> [BindingBackendOperation; 3] {
    [
        BindingBackendOperation::Delete,
        BindingBackendOperation::Get,
        BindingBackendOperation::Put,
    ]
}

const fn outcome(success: bool) -> &'static str {
    if success { "success" } else { "failure" }
}

pub(super) fn resource_operation_index(operation: ResourceOperation) -> usize {
    resource_operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}

pub(super) fn binding_operation_index(operation: BindingBackendOperation) -> usize {
    binding_operations()
        .iter()
        .position(|candidate| *candidate == operation)
        .unwrap()
}
