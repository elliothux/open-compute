//! Current restore-tree rejection cases, sharing the snapshot Gate's verified S3 fixture.

use open_compute_artifacts::SnapshotObjectStore;
use open_compute_core::{ErrorCode, PlatformSnapshotManifestV1};
use open_compute_storage::RestoreTarget;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

pub(super) async fn reject_invalid_staging(
    objects: &SnapshotObjectStore,
    manifest: &PlatformSnapshotManifestV1,
    root: &Path,
    key_fingerprint: &str,
) {
    for fault in ["extra-file", "file-mode", "master-key", "scheduler-schema"] {
        let target = root.join(format!("restore-reject-{fault}"));
        let restore = RestoreTarget::acquire(&target).expect("fresh restore target");
        for file in &manifest.files {
            let destination = restore
                .destination_for(&file.restore_path)
                .expect("restore path");
            objects
                .download_file(&file.object_key, &destination, &file.sha256, file.size)
                .await
                .expect("verified snapshot object");
        }
        let mut checked_manifest = manifest.clone();
        let mut expected_key = key_fingerprint.to_owned();
        let mut expected_error = ErrorCode::RestoreInvalid;
        match fault {
            "extra-file" => {
                let extra = restore
                    .destination_for("do/workerd/unexpected.bin")
                    .expect("extra path");
                super::write_mode(&extra, b"unexpected", 0o600);
            }
            "file-mode" => {
                fs::set_permissions(
                    restore.staging_root().join("control.sqlite"),
                    fs::Permissions::from_mode(0o644),
                )
                .expect("broad file mode");
                expected_error = ErrorCode::PathInvalid;
            }
            "master-key" => expected_key = "0".repeat(64),
            "scheduler-schema" => {
                checked_manifest
                    .source_schemas
                    .insert("scheduler".to_owned(), 99);
            }
            _ => unreachable!("fixed rejection cases"),
        }
        assert_eq!(
            restore
                .validate_and_publish(
                    &checked_manifest,
                    &expected_key,
                    5_000,
                    br#"{"schema_version":1}"#,
                )
                .expect_err(fault)
                .code(),
            expected_error,
            "{fault}"
        );
        assert!(!target.exists(), "{fault} must not publish any target");
    }
}
