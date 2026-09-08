//! Real pinned-workerd P0 aggregate Exit Gate.
//!
//! One Worker version owns every P0 product binding so cross-product composition, resource
//! isolation, backup/rebind, version fencing, process recovery, and failure isolation are
//! proven together rather than inferred from independent product Gates.

#![cfg(feature = "test-support")]

#[path = "../common/mod.rs"]
mod common;

#[path = "../p0_exit_support/mod.rs"]
mod support;

use common::load_file_only_platform_config;

use axum::http::StatusCode;
use open_compute_artifacts::{Fault, MockS3};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    BindingKind, ObjectStorageKind, RequestId, ResourceAvailability, ResourceId,
};
use open_compute_service::backup_cli::{
    backup_attest_restore_smoke, backup_create, backup_inspect, backup_restore,
};
use open_compute_service::doctor::{DoctorMode, doctor_report};
use open_compute_storage::{D1Migration, PlatformStorage, ResourceRepository, WorkerRepository};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeValidator, VersionController};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use support::{
    GateStack, ProductBindings, admin_json, admin_router, assert_public_id, assert_v4_envelope,
    capacity_summary, corrupt_d1, create_product_resource, deploy, dispatch, kill_workerd, now_ms,
    open_scheduler, repo_root, reset_capacity_samples, storage_config, stores, v4_product_ids,
    version_request, wait_pid_change,
};

mod config;
use config::*;
mod scenario;
use scenario::*;
mod products;
use products::*;
mod p0_real_combined_exit_matrix;

#[test]
fn p0_real_combined_exit_matrix() {
    p0_real_combined_exit_matrix::run();
}
