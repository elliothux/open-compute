//! Production `ocd` service library: config load, health, metrics, doctor, and run.

#![deny(missing_docs)]

pub mod ai_provider;
pub mod ai_search_backend;
pub mod ai_search_config;
pub mod ai_search_coordinator;
mod ai_tokenizer;
pub mod asset_backend;
pub mod auth;
mod backup_attestation;
pub mod backup_cli;
mod backup_retention;
pub mod binding_backend;
pub mod cache_backend;
pub(crate) mod cache_images_http;
pub mod capabilities;
pub mod cli;
pub mod config_load;
pub mod d1_backend;
pub mod d1_http;
mod d1_protocol;
mod d1_session;
pub mod dashboard;
#[cfg(test)]
mod dashboard_tests;
pub mod do_http;
pub mod doctor;
pub mod document_parser_backend;
pub mod embedded_dashboard;
pub mod exit;
pub mod health;
pub mod http;
pub mod images_backend;
pub mod kv_backend;
pub mod kv_http;
pub mod metrics;
mod operator_binding;
mod p2_3_promotion;
#[cfg(test)]
mod p3_3_test_support;
pub mod queue_backend;
pub mod queue_http;
pub mod r2_backend;
pub mod r2_http;
mod r2_maintenance;
mod r2_protocol;
mod resources;
pub mod run;
pub mod runtime_bridge;
pub mod runtime_generation;
pub mod scheduler;
pub mod scheduler_http;
pub mod search_http;
pub mod service_invocations;
mod snapshot_pins;
mod sqlite_staging;
pub mod support_bundle;
pub mod vectorize_backend;
pub mod vectorize_coordinator;
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

#[cfg(any(test, feature = "test-support"))]
pub use binding_backend::UnavailableKvBindingExecutor;
#[cfg(any(test, feature = "test-support"))]
pub use binding_backend::serve_binding_backend_with_ai_search;
pub use binding_backend::{
    KvBindingExecutor, bind_binding_backend, serve_binding_backend,
    serve_binding_backend_with_assets, serve_binding_backend_with_document_parser,
};
pub use cli::{Cli, Command, execute};
pub use d1_backend::D1BindingService;
pub use d1_http::D1ApiState;
pub use dashboard::{DashboardDispatch, bootstrap_dashboard};
pub use do_http::DoApiState;
pub use embedded_dashboard::embedded_dashboard_files;
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
pub use search_http::SearchApiState;

#[cfg(test)]
mod tests;
