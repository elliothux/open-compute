//! Durable Workflow behavior through the production scheduler, binding dispatcher, and stock workerd.

#![cfg(feature = "test-support")]

#[path = "workflow_support/durable_batches.rs"]
mod durable_batches;
#[path = "workflow_support/durable_execution.rs"]
mod durable_execution;
mod workflow_support;

use open_compute_core::{SchedulerClock as _, SystemSchedulerClock};
use workflow_support::Harness;

fn now() -> i64 {
    SystemSchedulerClock.wall_time_ms()
}

fn start_backend(
    harness: &mut Harness,
    store: &std::sync::Arc<open_compute_storage::SchedulerStore>,
    limits: &open_compute_core::WorkflowsConfig,
    metrics: &std::sync::Arc<open_compute_service::metrics::MetricsRegistry>,
) -> tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>> {
    use std::sync::Arc;
    let auth = harness.binding_auth.clone();
    let listener = harness.binding_listener.take().unwrap();
    let mut shutdown = harness.shutdown.subscribe();
    tokio::spawn(
        open_compute_service::binding_backend::serve_binding_backend_with_scheduler(
            listener,
            harness.storage.clone(),
            auth,
            open_compute_workers::ResourcePins::new(),
            Arc::new(
                open_compute_service::kv_backend::SqliteKvBindingExecutor::new(
                    harness.storage.clone(),
                    Arc::new(open_compute_core::SystemClock),
                ),
            ),
            Some(metrics.clone()),
            None,
            None,
            Default::default(),
            Default::default(),
            limits.clone(),
            Some(store.clone()),
            async move {
                let _ = shutdown.changed().await;
            },
        ),
    )
}
