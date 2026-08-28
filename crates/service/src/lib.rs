//! Production `platformd` service library: config load, health, metrics, doctor, and run.

#![deny(missing_docs)]

pub mod auth;
mod backup_attestation;
pub mod backup_cli;
mod backup_retention;
pub mod binding_backend;
pub mod capabilities;
pub mod cli;
pub mod config_load;
pub mod d1_backend;
pub mod d1_http;
mod d1_protocol;
pub mod do_http;
pub mod doctor;
pub mod exit;
pub mod health;
pub mod http;
pub mod kv_backend;
pub mod kv_http;
pub mod metrics;
mod p2_3_promotion;
pub mod queue_backend;
pub mod queue_http;
pub mod r2_backend;
pub mod r2_http;
mod r2_maintenance;
mod r2_protocol;
pub mod run;
pub mod runtime_bridge;
pub mod scheduler;
pub mod scheduler_http;
mod snapshot_pins;
pub mod support_bundle;
pub mod upgrade_cli;
mod worker_cli;
pub mod workers_http;
pub mod workflow_backend;
pub mod workflow_http;

/// Compose the production promotion owner for real-process integration fixtures.
/// This entry point is absent from ordinary production builds.
#[cfg(feature = "test-support")]
#[must_use]
pub fn product_promotion_for_test(
    storage: std::sync::Arc<open_compute_storage::PlatformStorage>,
    scheduler: std::sync::Arc<open_compute_storage::SchedulerStore>,
) -> std::sync::Arc<dyn open_compute_workers::ProductPromotionCoordinator> {
    std::sync::Arc::new(p2_3_promotion::P23PromotionCoordinator::new(
        storage,
        scheduler,
        std::time::Duration::from_secs(1),
    ))
}

pub use binding_backend::{
    KvBindingExecutor, UnavailableKvBindingExecutor, bind_binding_backend, serve_binding_backend,
    serve_binding_backend_with_metrics, serve_binding_backend_with_products,
    serve_binding_backend_with_products_and_do_config, serve_binding_backend_with_r2,
    serve_binding_backend_with_scheduler,
};
pub use cli::{Cli, Command, execute};
pub use d1_backend::D1BindingService;
pub use d1_http::D1ApiState;
pub use do_http::DoApiState;
pub use exit::{ExitClass, emit_failure, exit_code};
pub use health::{HealthCoordinator, map_supervisor};
pub use kv_backend::{KvCommand, KvCommandResult, KvStreamPart, SqliteKvBindingExecutor};
pub use kv_http::KvApiState;
pub use metrics::MetricsRegistry;
pub use r2_backend::R2BindingService;
pub use r2_http::R2ApiState;
pub use run::run_platform;
#[cfg(any(test, feature = "test-support"))]
pub use run::{FailAfter, RunOptions, run_platform_with};
pub use scheduler::SchedulerService;

#[cfg(test)]
mod tests;
