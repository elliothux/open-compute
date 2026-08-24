//! Production `platformd` service library: config load, health, metrics, doctor, and run.

#![deny(missing_docs)]

pub mod auth;
pub mod binding_backend;
pub mod cli;
pub mod config_load;
pub mod doctor;
pub mod exit;
pub mod health;
pub mod http;
pub mod metrics;
pub mod run;
pub mod runtime_bridge;
pub mod workers_http;

pub use binding_backend::{
    KvBindingExecutor, UnavailableKvBindingExecutor, bind_binding_backend, serve_binding_backend,
    serve_binding_backend_with_metrics,
};
pub use cli::{Cli, Command, execute};
pub use exit::{ExitClass, emit_failure, exit_code};
pub use health::{HealthCoordinator, map_supervisor};
pub use metrics::MetricsRegistry;
pub use run::run_platform;
#[cfg(any(test, feature = "test-support"))]
pub use run::{FailAfter, RunOptions, run_platform_with};

#[cfg(test)]
mod tests;
