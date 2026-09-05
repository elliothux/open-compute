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
mod cloudflare_v4;
pub mod config_load;
mod d1_api;
pub mod d1_backend;
mod d1_backup;
mod d1_coordinator;
mod d1_protocol;
mod d1_session;
pub mod dashboard;
#[cfg(test)]
mod dashboard_tests;
pub mod do_lifecycle;
pub mod doctor;
pub mod document_parser_backend;
pub mod embedded_dashboard;
pub mod exit;
pub mod health;
pub mod http;
pub mod images_backend;
pub mod kv_api;
pub mod kv_backend;
pub mod metrics;
mod object_storage;
mod observability;
mod observability_backend;
mod observability_filter;
mod p2_3_promotion;
#[cfg(test)]
mod p3_3_test_support;
mod queue_api;
pub mod queue_backend;
pub mod r2_api;
pub mod r2_backend;
mod r2_maintenance;
mod r2_protocol;
mod resource_binding;
mod resources;
pub mod run;
pub mod runtime_bridge;
pub mod runtime_generation;
pub mod scheduler;
pub mod search_api;
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

/// Attach the production Cloudflare v4 account mapping to an HTTP test state.
///
/// The returned account identifier is the public salted identifier that fixed
/// Cloudflare clients must place in both configuration and request paths. This
/// entry point is absent from ordinary production builds.
#[cfg(feature = "test-support")]
#[must_use]
pub fn cloudflare_v4_for_test(
    state: http::HttpState,
    storage: std::sync::Arc<open_compute_storage::PlatformStorage>,
) -> (http::HttpState, String) {
    let authority = cloudflare_v4::accounts::AccountAuthority::new(
        storage.identity().platform_id,
        storage.identity().default_account_id,
        storage.identity().created_at_ms,
    );
    let public_id = authority.public_id().to_owned();
    (
        state
            .with_cloudflare_v4_account(authority)
            .with_platform_storage(storage),
        public_id,
    )
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
pub use d1_api::D1ApiState;
pub use d1_backend::D1BindingService;
pub use dashboard::{DashboardDispatch, bootstrap_dashboard};
pub use do_lifecycle::DurableObjectLifecycleService;
pub use embedded_dashboard::embedded_dashboard_files;
pub use exit::{ExitClass, emit_failure, exit_code};
pub use health::{HealthCoordinator, map_supervisor};
pub use kv_api::KvApiState;
pub use kv_backend::{KvCommand, KvCommandResult, KvStreamPart, SqliteKvBindingExecutor};
pub use metrics::MetricsRegistry;
pub use queue_api::QueueApiState;
pub use r2_api::R2ApiState;
pub use r2_backend::R2BindingService;
pub use run::run_platform;
#[cfg(any(test, feature = "test-support"))]
pub use run::{FailAfter, RunOptions, run_platform_with};
pub use scheduler::SchedulerService;
pub use search_api::SearchApiState;
#[cfg(any(test, feature = "test-support"))]
pub use vectorize_coordinator::VectorizeCoordinator;

#[cfg(test)]
mod tests;
