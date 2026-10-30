//! Directory handles from the real native producer restore through all stable mount forms.

use super::*;
use open_compute_artifacts::{ArtifactRef, ContainerImageLimits, verify_container_image_archive};
use open_compute_storage::containers::{ContainerSnapshot, ContainerSnapshotState};
use std::io::Read as _;
use std::path::Path;

const SOURCE: &str = "/tmp/p23-directory-state";
const COPY: &str = "/tmp/p23-directory-copy";

pub(super) struct SavedDirectories {
    handles: [Value; 2],
    records: [ContainerSnapshot; 2],
    default_handle: Value,
    default_record: ContainerSnapshot,
}

fn id(handle: &Value) -> ResourceId {
    handle["id"].as_str().unwrap().parse().unwrap()
}

fn assert_tree(value: &Value, path: &str, expected: &str, child_files: &Value) {
    assert_eq!(
        value[path],
        json!({
            "value": expected, "aliasValue": expected, "mode": 0o640,
            "uid": 1001, "gid": 1002, "mtime": 1_700_000_000,
            "hardlink": true, "symlink": true, "symlinkTarget": "../elsewhere",
            "symlinkMetadata": {"uid": 1001, "gid": 1002, "mtime": 1_700_000_000},
            "directoryMetadata": {"mode": 0o750, "uid": 1001, "gid": 1002, "mtime": 1_700_000_000},
            "additionalLinks": {
                "deep-link": {"target": "../../../../p23-directory-dangling", "symlink": true, "uid": 1001, "gid": 1002, "mtime": 1_700_000_000},
                "absolute-link": {"target": "/p23-directory-dangling", "symlink": true, "uid": 1001, "gid": 1002, "mtime": 1_700_000_000},
            },
            "childFiles": child_files,
        })
    );
}

fn assert_restored(value: &Value) {
    assert_tree(value, SOURCE, "saved", &json!(["nested.txt"]));
    assert_tree(value, COPY, "saved", &json!([]));
    assert_eq!(value["empty"], json!([]));
    assert_eq!(value.as_object().unwrap().len(), 3);
}

async fn post(
    client: &http::Client,
    public: &str,
    host: &str,
    object: &str,
    action: &str,
    handle: &Value,
) -> Value {
    http::json(
        client,
        public,
        host,
        &format!("/named/directory/{action}?object={object}"),
        "POST",
        None,
        handle.clone(),
    )
    .await
}

async fn stop(client: &http::Client, public: &str, host: &str, object: &str) {
    assert_eq!(
        tenant(
            client,
            public,
            host,
            &format!("/named/directory/stop?object={object}")
        )
        .await,
        json!({"running": false})
    );
}

fn restored_lease(
    data: &Path,
    instance: InstanceId,
    application: ResourceId,
    snapshot: ResourceId,
) -> ContainerLease {
    let lease = leases(data, instance, application)
        .into_iter()
        .find(|lease| {
            lease.state == ContainerLeaseState::Running
                && lease
                    .directory_snapshots
                    .iter()
                    .any(|entry| entry.snapshot_id == Some(snapshot))
        })
        .unwrap();
    assert!(lease.startup.snapshot_id.is_none());
    assert_eq!(
        serde_json::to_value(&lease.directory_snapshots).unwrap(),
        json!([
            {"snapshotId": snapshot, "mountPoint": SOURCE},
            {"snapshotId": snapshot, "mountPoint": COPY},
            {"snapshotId": null, "mountPoint": "/tmp/p23-directory-empty"},
            {"snapshotId": null, "mountPoint": "/tmp/p23-directory-copy/child"},
        ])
    );
    lease
}

async fn assert_artifact(
    config: &PlatformConfig,
    instance: InstanceId,
    record: &ContainerSnapshot,
    handle: &Value,
) {
    assert_eq!(record.state, ContainerSnapshotState::Ready);
    assert_eq!(record.directory.as_ref().unwrap().as_str(), SOURCE);
    assert_eq!(record.id, id(handle));
    assert_eq!(record.image_size, handle["size"].as_u64());
    assert!(handle["size"].as_u64().unwrap() > 0);
    let image = {
        let db = ControlDb::open_readonly_wal_aware(&config.data.path.join("control.sqlite"), 1000)
            .unwrap();
        ContainerAuthority::new(&db)
            .image(instance, &record.image_sha256.unwrap())
            .unwrap()
    };
    let artifact = ArtifactRef::new(
        open_compute_artifacts::ARTIFACT_KEY_VERSION,
        &hex::encode(record.artifact_sha256.unwrap()),
        record.artifact_size.unwrap(),
    )
    .unwrap();
    let file = snapshots::persisted_artifact(config, record).await;
    let public_size = handle["size"].as_u64().unwrap();
    tokio::task::spawn_blocking(move || {
        let verified = verify_container_image_archive(
            &file,
            &artifact,
            image.manifest_sha256,
            &image.architecture,
            ContainerImageLimits {
                json_bytes: 1 << 20,
                compressed_bytes: image.compressed_bytes,
                unpacked_bytes: image.unpacked_bytes,
                layers: 1,
                entries: 100_000,
            },
        )
        .unwrap();
        assert_eq!(verified.layers().len(), 1);
        let layer = &verified.layers()[0];
        let expected_path = format!("blobs/sha256/{}", hex::encode(layer.sha256));
        let mut found = 0;
        let mut archive = tar::Archive::new(file);
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            if entry.path_bytes().as_ref() != expected_path.as_bytes() {
                continue;
            }
            let mut decoded = flate2::read::MultiGzDecoder::new(entry).take((2 << 20) + 1);
            let size = std::io::copy(&mut decoded, &mut std::io::sink()).unwrap();
            assert!(
                size <= 2 << 20,
                "owned directory archive exceeded its fixture bound"
            );
            assert_eq!(
                size, public_size,
                "directory size is the complete captured tar byte count"
            );
            found += 1;
        }
        assert_eq!(found, 1);
    })
    .await
    .unwrap();
}

pub(super) async fn capture(
    client: &http::Client,
    public: &str,
    host: &str,
    config: &Path,
    scope: (InstanceId, ResourceId, ResourceId),
    provider: &QualifiedContainerProvider,
) -> SavedDirectories {
    let (instance, application, default_application) = scope;
    let config = PlatformConfig::from_toml_str(&fs::read_to_string(config).unwrap()).unwrap();
    let data = &config.data.path;
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/named/directory/surface?object=directory-source"
        )
        .await,
        json!({"capture": true})
    );
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/named/directory/write?object=directory-source"
        )
        .await,
        json!({"written": "saved"})
    );
    let source = leases(data, instance, application)
        .into_iter()
        .find(|lease| lease.state == ContainerLeaseState::Running)
        .unwrap();
    let before = provider.observe(&source.opaque_engine_key).await.unwrap();
    let named = tenant(
        client,
        public,
        host,
        "/named/directory/capture?object=directory-source",
    )
    .await;
    let unnamed = tenant(
        client,
        public,
        host,
        "/named/directory/capture?object=directory-source&unnamed=1",
    )
    .await;
    assert_eq!(named.as_object().unwrap().len(), 4);
    assert_eq!(named["name"], "directory-checkpoint");
    assert_eq!(unnamed.as_object().unwrap().len(), 3);
    assert!(unnamed.get("name").is_none());
    assert_ne!(id(&named), id(&unnamed));
    let handles = [named, unnamed];
    let records = handles
        .each_ref()
        .map(|handle| snapshots::snapshot(data, instance, id(handle)));
    for (handle, record) in handles.iter().zip(&records) {
        assert_eq!(handle["dir"], SOURCE);
        assert_eq!(record.application_id, application);
        assert_eq!(record.source.identity, source.identity);
        assert_artifact(&config, instance, record, handle).await;
    }
    let after = provider.observe(&source.opaque_engine_key).await.unwrap();
    assert!(after.running && !after.late_operations_fenced);
    assert_eq!(after.start_identity, before.start_identity);
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/named/directory/mutate?object=directory-source"
        )
        .await,
        json!({"written": "mutated"})
    );
    let mutated = tenant(
        client,
        public,
        host,
        "/named/directory/read?object=directory-source&source=1",
    )
    .await;
    assert_tree(&mutated, SOURCE, "mutated", &json!(["nested.txt"]));
    stop(client, public, host, "directory-source").await;
    let (default_handle, default_record) = capture_default_policy(
        client,
        public,
        host,
        &config,
        (instance, default_application),
        provider,
    )
    .await;
    SavedDirectories {
        handles,
        records,
        default_handle,
        default_record,
    }
}

pub(super) async fn cross_restore(
    client: &http::Client,
    public: &str,
    host: &str,
    config: &Path,
    scope: (InstanceId, ResourceId, ResourceId, ResourceId),
    provider: &QualifiedContainerProvider,
    saved: &SavedDirectories,
) {
    let (instance, application, default_application, full_snapshot) = scope;
    let config = PlatformConfig::from_toml_str(&fs::read_to_string(config).unwrap()).unwrap();
    let data = &config.data.path;
    let handles = &saved.handles;
    let records = &saved.records;
    let default_handle = &saved.default_handle;
    for path in [
        "/named/directory/surface?object=directory-stable-surface",
        "/inactivity/directory/surface?object=directory-default-stable-surface",
    ] {
        assert_eq!(
            tenant(client, public, host, path).await,
            json!({"capture": false})
        );
    }
    for (handle, record) in handles.iter().zip(records) {
        assert_restored(
            &post(
                client,
                public,
                host,
                "directory-target",
                "restore-stream",
                handle,
            )
            .await,
        );
        let target = restored_lease(data, instance, application, record.id);
        assert_ne!(target.identity.object_id, record.source.identity.object_id);
        assert_eq!(target.effective_image_sha256, record.source_image_sha256);
        assert_eq!(
            tenant(
                client,
                public,
                host,
                "/named/directory/mutate?object=directory-target"
            )
            .await,
            json!({"written": "mutated"})
        );
        let replay = post(client, public, host, "directory-target", "replay", handle).await;
        assert_eq!(replay["startRejected"], true);
        assert_tree(&replay, SOURCE, "mutated", &json!(["nested.txt"]));
        assert_tree(&replay, COPY, "saved", &json!([]));
        assert_eq!(replay["empty"], json!([]));
        let same = restored_lease(data, instance, application, record.id);
        assert_eq!(same.identity, target.identity);
        assert_eq!(same.opaque_engine_key, target.opaque_engine_key);
        assert_eq!(same.start_identity, target.start_identity);
        stop(client, public, host, "directory-target").await;
    }
    let count = leases(data, instance, application).len();
    let mut wrong_kind = handles[0].clone();
    wrong_kind["id"] = json!(full_snapshot);
    let (status, _, _) = http::request(
        client,
        public,
        host,
        "/named/directory/restore?object=directory-wrong-kind",
        "POST",
        None,
        "application/json",
        serde_json::to_vec(&wrong_kind).unwrap(),
    )
    .await;
    assert_eq!(status, 500);
    let (status, _, _) = http::request(
        client,
        public,
        host,
        &format!(
            "/named/restore?object=directory-as-full&id={}",
            records[0].id
        ),
        "GET",
        None,
        "application/json",
        Vec::new(),
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(
        leases(data, instance, application).len(),
        count,
        "wrong-kind snapshot handles must fail before reserving a lease"
    );
    named_images::assert_stopped(data, instance, application, provider).await;
    let record = &saved.default_record;
    let source = &record.source;
    let restored = http::json(
        client,
        public,
        host,
        "/inactivity/directory/restore-stream?object=directory-default-target",
        "POST",
        None,
        saved.default_handle.clone(),
    )
    .await;
    assert_restored(&restored);
    let target = restored_lease(data, instance, default_application, record.id);
    assert_ne!(target.identity.object_id, source.identity.object_id);
    assert_eq!(target.effective_image_sha256, record.source_image_sha256);
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/inactivity/directory/stop?object=directory-default-target"
        )
        .await,
        json!({"running": false})
    );
    named_images::assert_stopped(data, instance, default_application, provider).await;
    let counts = [application, default_application].map(|app| leases(data, instance, app).len());
    for (path, foreign) in [
        (
            "/inactivity/directory/restore?object=directory-foreign-named",
            &handles[0],
        ),
        (
            "/named/directory/restore?object=directory-foreign-default",
            default_handle,
        ),
    ] {
        let (status, _, _) = http::request(
            client,
            public,
            host,
            path,
            "POST",
            None,
            "application/json",
            serde_json::to_vec(foreign).unwrap(),
        )
        .await;
        assert_eq!(status, 500);
    }
    assert_eq!(
        [application, default_application].map(|app| leases(data, instance, app).len()),
        counts,
        "cross-application handles must fail before reserving a lease"
    );
}

async fn capture_default_policy(
    client: &http::Client,
    public: &str,
    host: &str,
    config: &PlatformConfig,
    scope: (InstanceId, ResourceId),
    provider: &QualifiedContainerProvider,
) -> (Value, ContainerSnapshot) {
    let (instance, application) = scope;
    let data = &config.data.path;
    named_images::assert_stopped(data, instance, application, provider).await;
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/inactivity/directory/surface?object=directory-default-source"
        )
        .await,
        json!({"capture": true})
    );
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/inactivity/directory/write?object=directory-default-source"
        )
        .await,
        json!({"written": "saved"})
    );
    let source = leases(data, instance, application)
        .into_iter()
        .find(|lease| lease.state == ContainerLeaseState::Running)
        .unwrap();
    let before = provider.observe(&source.opaque_engine_key).await.unwrap();
    let handle = tenant(
        client,
        public,
        host,
        "/inactivity/directory/capture?object=directory-default-source&unnamed=1",
    )
    .await;
    assert_eq!(handle.as_object().unwrap().len(), 3);
    assert_eq!(handle["dir"], SOURCE);
    assert!(handle.get("name").is_none());
    let record = snapshots::snapshot(data, instance, id(&handle));
    assert_eq!(record.application_id, application);
    assert_eq!(record.source.identity, source.identity);
    assert_artifact(config, instance, &record, &handle).await;
    let after = provider.observe(&source.opaque_engine_key).await.unwrap();
    assert!(after.running && !after.late_operations_fenced);
    assert_eq!(before.start_identity, after.start_identity);
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/inactivity/directory/stop?object=directory-default-source"
        )
        .await,
        json!({"running": false})
    );
    named_images::assert_stopped(data, instance, application, provider).await;
    (handle, record)
}

pub(super) async fn restore_after_restart(
    client: &http::Client,
    public: &str,
    host: &str,
    config: &Path,
    instance: InstanceId,
    applications: (ResourceId, ResourceId),
    saved: SavedDirectories,
) {
    let (application, default_application) = applications;
    let config = PlatformConfig::from_toml_str(&fs::read_to_string(config).unwrap()).unwrap();
    let provider = QualifiedContainerProvider::preflight(config.containers.as_ref().unwrap())
        .await
        .unwrap();
    for (handle, prior) in saved.handles.iter().zip(&saved.records) {
        let before = snapshots::snapshot(&config.data.path, instance, prior.id);
        assert_eq!(before.artifact_sha256, prior.artifact_sha256);
        assert_eq!(before.artifact_size, prior.artifact_size);
        assert_eq!(before.image_sha256, prior.image_sha256);
        assert_eq!(before.directory, prior.directory);
        assert_artifact(&config, instance, &before, handle).await;
        assert_restored(
            &post(
                client,
                public,
                host,
                "directory-persisted",
                "restore-stream",
                handle,
            )
            .await,
        );
        let target = restored_lease(&config.data.path, instance, application, prior.id);
        assert_ne!(target.identity.object_id, prior.source.identity.object_id);
        let renewed = snapshots::snapshot(&config.data.path, instance, prior.id);
        assert!(renewed.last_restored_at_ms.unwrap() >= prior.captured_at_ms.unwrap());
        assert_eq!(
            renewed.expires_at_ms,
            renewed.last_restored_at_ms.unwrap()
                + open_compute_storage::containers::CONTAINER_SNAPSHOT_RETENTION_MS
        );
        stop(client, public, host, "directory-persisted").await;
    }
    named_images::assert_stopped(&config.data.path, instance, application, &provider).await;
    let prior = saved.default_record;
    let before = snapshots::snapshot(&config.data.path, instance, prior.id);
    assert_eq!(before.artifact_sha256, prior.artifact_sha256);
    assert_eq!(before.artifact_size, prior.artifact_size);
    assert_eq!(before.directory, prior.directory);
    assert_artifact(&config, instance, &before, &saved.default_handle).await;
    assert_restored(
        &http::json(
            client,
            public,
            host,
            "/inactivity/directory/restore-stream?object=directory-default-persisted",
            "POST",
            None,
            saved.default_handle,
        )
        .await,
    );
    let target = restored_lease(&config.data.path, instance, default_application, prior.id);
    assert_ne!(target.identity.object_id, prior.source.identity.object_id);
    let renewed = snapshots::snapshot(&config.data.path, instance, prior.id);
    assert_eq!(
        renewed.expires_at_ms,
        renewed.last_restored_at_ms.unwrap()
            + open_compute_storage::containers::CONTAINER_SNAPSHOT_RETENTION_MS
    );
    assert_eq!(
        tenant(
            client,
            public,
            host,
            "/inactivity/directory/stop?object=directory-default-persisted"
        )
        .await,
        json!({"running": false})
    );
    named_images::assert_stopped(&config.data.path, instance, default_application, &provider).await;
}
