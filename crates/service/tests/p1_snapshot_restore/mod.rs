//! P1.2/P1.3 full snapshot and fresh-host restore integration Gate.

#[path = "../common/mod.rs"]
mod common;
mod p5_search;
mod staged_validation;

use common::load_file_only_platform_config;

use base64::Engine as _;
use open_compute_artifacts::{
    AiSearchObjectStore, MockS3, ObjectBackend, SnapshotObjectStore, resolve_s3_credentials,
};
use open_compute_core::{
    CronActivationId, ErrorCode, PlatformSnapshotManifestV1, RequestId, SystemClock, VersionId,
    WorkerId,
};
use open_compute_service::backup_cli::{
    backup_attest_restore_smoke, backup_create, backup_delete, backup_inspect, backup_list,
    backup_restore, backup_retention_plan,
};
use open_compute_service::cli::{execute, parse_from};
use open_compute_storage::{
    PlatformStorage, QueueConfig, QueueContentType, QueueEnqueueRequest, QueueMessageInput,
    QueueRepository, RestoreTarget, SchedulerStore, inspect_control_db, inspect_master_key,
    inspect_scheduler_db, sign_snapshot_manifest,
};
use open_compute_workers::{CreateQueueOutcome, CreateQueueRequest, QueueController};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

mod config;
use config::*;
mod scenario;
use scenario::*;
mod p1_full_snapshot_retention_and_fresh_host_restore_are_fail_closed;

#[test]
fn p1_full_snapshot_retention_and_fresh_host_restore_are_fail_closed() {
    p1_full_snapshot_retention_and_fresh_host_restore_are_fail_closed::run();
}
