//! Fixed Wrangler resource commands against the real local v4 composition.

#![cfg(feature = "test-support")]

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
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, VersionContentKind, WorkerRepository,
    WorkflowRepository,
};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WRANGLER_VERSION: &str = "4.127.1";
const ADMIN_TOKEN: &str = platform_process::ADMIN_TOKEN;
const TOKEN: &str = "p6-wrangler-resource-gate-deployer-token";
const READ_ONLY_TOKEN: &str = "p6-wrangler-resource-gate-read-only-token";
const TAIL_SECRET: &str = "p7-tail-secret-value";
const S3_ACCESS_KEY: &str = "AKIAEXAMPLEKEYID01";
const S3_SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
const KV_NAME: &str = "resource-gate-kv";
const D1_NAME: &str = "resource-gate-d1";
const R2_NAME: &str = "resource-gate-r2";
const QUEUE_NAME: &str = "resource-gate-queue";
const WORKFLOW_NAME: &str = "resource-gate-workflow";

mod fixed_wrangler_resource_commands_use_live_v4_authorities;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_wrangler_resource_commands_use_live_v4_authorities() {
    fixed_wrangler_resource_commands_use_live_v4_authorities::run().await;
}

mod tail;
use tail::*;
mod products;
use products::*;
mod fixture;
use fixture::*;
mod setup;
use setup::*;
mod wrangler;
use wrangler::*;
