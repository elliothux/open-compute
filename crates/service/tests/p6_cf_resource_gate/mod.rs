//! Fixed cf resource commands against the real local v4 composition.

#![cfg(feature = "test-support")]

mod artifacts;
mod evidence;
mod search;
mod worker_loader;

#[allow(
    dead_code,
    reason = "this Gate reuses the production-process ownership half of the Workflow fixture"
)]
#[path = "../workflow_support/platform_process.rs"]
mod platform_process;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use evidence::Evidence;
use open_compute_artifacts::MockS3;
use open_compute_core::config::DataConfig;
use open_compute_core::{Redactor, RequestId, SystemClock, VersionId};
use open_compute_runtime::verify_runtime_binary;
use open_compute_storage::PlatformStorage;
use open_compute_storage::worker_repository::{
    NewVersion, NewVersionProducts, VersionContentKind, WorkerRepository,
};
use open_compute_storage::workflows::WorkflowRepository;
use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CF_VERSION: &str = "1.0.0-beta.21";
const ADMIN_TOKEN: &str = platform_process::ADMIN_TOKEN;
const TOKEN: &str = "p6-cf-resource-gate-deployer-token";
const READ_ONLY_TOKEN: &str = "p6-cf-resource-gate-read-only-token";
const TAIL_SECRET: &str = "p7-tail-secret-value";
const S3_ACCESS_KEY: &str = "AKIAEXAMPLEKEYID01";
const S3_SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
const KV_NAME: &str = "resource-gate-kv";
const D1_NAME: &str = "resource-gate-d1";
const R2_NAME: &str = "resource-gate-r2";
const QUEUE_NAME: &str = "resource-gate-queue";
const WORKFLOW_NAME: &str = "resource-gate-workflow";

fn worker_host(account: &str, worker: &str) -> String {
    format!("{worker}.{account}.localhost")
}

mod fixed_cf_resource_commands_use_live_v4_authorities;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_cf_resource_commands_use_live_v4_authorities() {
    fixed_cf_resource_commands_use_live_v4_authorities::run().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w2_cf_limits_settings_clone_and_restart() {
    worker_loader::resource_limits_settings_clone_and_restart().await;
}

mod tail;
use tail::*;
mod products;
use products::*;
mod p20;
use p20::*;
mod fixture;
use fixture::*;
mod setup;
use setup::*;
mod cf;
use cf::*;
