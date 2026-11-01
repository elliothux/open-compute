//! Application-owned snapshot capture; SQLite fences every external publication.

use super::*;
use open_compute_artifacts::{ArtifactRef, ContainerImageLimits};
use open_compute_storage::containers::{
    ContainerImage, ContainerSnapshotLimits, ContainerSnapshotSource, NewContainerSnapshot,
};
use std::io::{Read, Seek};
use std::os::unix::fs::PermissionsExt;

mod retirement;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SnapshotRequest {
    pub source: StopRequest,
    pub name: Option<String>,
    pub directory: Option<open_compute_core::containers::ContainerDirectoryPath>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotResponse {
    pub snapshot: SnapshotMetadata,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotMetadata {
    pub id: ResourceId,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<open_compute_core::containers::ContainerDirectoryPath>,
}

struct CaptureGuard {
    service: Arc<ContainerService>,
    id: ResourceId,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.service
            .snapshot_captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl ContainerService {
    /// Finish admitted publication independently of a disconnected runtime request.
    pub(crate) async fn capture_snapshot(
        self: &Arc<Self>,
        request: SnapshotRequest,
    ) -> Result<SnapshotResponse, PlatformError> {
        let service = Arc::clone(self);
        tokio::spawn(async move { service.capture_snapshot_inner(request).await })
            .await
            .map_err(|_| unavailable())?
    }

    async fn capture_snapshot_inner(
        self: &Arc<Self>,
        request: SnapshotRequest,
    ) -> Result<SnapshotResponse, PlatformError> {
        #[cfg(feature = "test-support")]
        let diagnostic_started = std::time::Instant::now();
        #[cfg(feature = "test-support")]
        let diagnostic_phase = move |phase: &'static str| {
            tracing::info!(
                phase,
                elapsed_ms = %diagnostic_started.elapsed().as_millis(),
                "Controlled snapshot capture phase"
            );
        };
        let authority = self.request_authority(&request.source)?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("authority-validated");
        let repo = ContainerAuthority::new(self.storage.db());
        let app = repo.application_for_namespace(
            self.instance_id(),
            authority.namespace_resource_id,
            &request.source.application_name,
        )?;
        if request.directory.is_none()
            && app.scheduling_policy
                != open_compute_storage::containers::ContainerSchedulingPolicy::DurableObject
        {
            return Err(wire::invalid());
        }
        #[cfg(feature = "test-support")]
        diagnostic_phase("application-policy-validated");
        let lease = repo.lease(self.instance_id(), request.source.identity())?;
        if lease.opaque_engine_key != request.source.opaque_engine_key {
            return Err(unavailable());
        }
        let source = ContainerSnapshotSource {
            identity: lease.identity,
            opaque_engine_key: lease.opaque_engine_key.clone(),
            start_identity: lease.start_identity.clone().ok_or_else(unavailable)?,
        };
        #[cfg(feature = "test-support")]
        diagnostic_phase("source-lease-validated");
        let budget = self.provider.max_image_bytes();
        let rootfs_budget = budget
            .checked_sub(4 << 20)
            .filter(|n| *n > 0)
            .ok_or_else(unavailable)?;
        let original = self.admitted_image(&lease.effective_image_sha256)?;
        let artifact = image_artifact(&original)?;
        let disk_bytes = budget
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(original.artifact_size))
            .ok_or_else(unavailable)?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("source-image-validated");
        let capture_disk = self.storage.reserve_mutation(disk_bytes)?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("disk-reserved");
        let id = ResourceId::generate();
        let guard = CaptureGuard {
            service: Arc::clone(self),
            id,
        };
        {
            let mut active = self
                .snapshot_captures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            repo.reserve_snapshot(
                &authority,
                &NewContainerSnapshot {
                    directory: request.directory.clone(),
                    id,
                    source: source.clone(),
                    name: request.name,
                    reserved_bytes: budget,
                },
                ContainerSnapshotLimits {
                    max_snapshots: self.provider.max_snapshots(),
                    max_bytes: self.provider.max_snapshot_bytes(),
                },
                open_compute_core::wall_time_ms(),
            )?;
            active.insert(id);
        }
        #[cfg(feature = "test-support")]
        diagnostic_phase("snapshot-reserved");
        // Keep the GC reservation inside this task until PUT and its authority commit finish.
        let _gc = self.artifacts.reserve_version_artifact().await;
        self.ensure_active().await?;
        let pin = self.cache.acquire(&self.artifacts, &artifact).await?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("source-artifact-pinned");
        let temporary = std::fs::canonicalize(self.storage.data_dir().prepare_tmp_dir()?)
            .map_err(|_| unavailable())?;
        let configuration = tokio::task::spawn_blocking(move || {
            let source_limits = image_limits(&original);
            let configuration = open_compute_artifacts::read_container_image_configuration(
                pin.file(),
                &artifact,
                original.manifest_sha256,
                &original.architecture,
                source_limits,
            )?;
            #[cfg(feature = "test-support")]
            diagnostic_phase("source-configuration-verified");
            Ok::<_, PlatformError>(configuration)
        });
        let capture = async {
            let captured = match &request.directory {
                Some(directory) => {
                    self.provider
                        .capture_directory_snapshot(
                            &source.opaque_engine_key,
                            &source.start_identity,
                            rootfs_budget,
                            &temporary,
                            directory,
                        )
                        .await?
                }
                None => {
                    self.provider
                        .capture_snapshot(
                            &source.opaque_engine_key,
                            &source.start_identity,
                            rootfs_budget,
                            &temporary,
                        )
                        .await?
                }
            };
            #[cfg(feature = "test-support")]
            diagnostic_phase("rootfs-capture-ready");
            Ok::<_, PlatformError>(captured)
        };
        // Both operations own resources that must finish before the reservation is released.
        let (configuration, captured) = tokio::join!(configuration, capture);
        let (rootfs, captured_bytes) = captured?;
        let (verified, configuration) = configuration.map_err(|_| unavailable())??;
        let (image, output) = tokio::task::spawn_blocking(move || {
            let output = tempfile::tempfile_in(&temporary).map_err(|_| unavailable())?;
            let compressed = tempfile::tempfile_in(temporary).map_err(|_| unavailable())?;
            // Linux anonymous files can inherit a permissive umask.
            output
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| unavailable())?;
            compressed
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| unavailable())?;
            let limits = ContainerImageLimits {
                json_bytes: 1 << 20,
                compressed_bytes: rootfs_budget,
                unpacked_bytes: rootfs_budget,
                layers: 1,
                entries: 100_000,
            };
            let (verified, output) = open_compute_artifacts::write_container_snapshot_image(
                &verified,
                &configuration,
                &rootfs,
                compressed,
                limits,
                std::io::BufWriter::with_capacity(256 * 1024, output),
            )?;
            let mut output = output.into_inner().map_err(|_| unavailable())?;
            #[cfg(feature = "test-support")]
            diagnostic_phase("snapshot-archive-written");
            output.sync_all().map_err(|_| unavailable())?;
            let size = output.metadata().map_err(|_| unavailable())?.len();
            if size == 0 || size > budget {
                return Err(unavailable());
            }
            output.rewind().map_err(|_| unavailable())?;
            let mut digest = Sha256::new();
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = output.read(&mut buffer).map_err(|_| unavailable())?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            Ok::<_, PlatformError>((
                ContainerImage {
                    manifest_sha256: *verified.manifest_sha256(),
                    config_sha256: verified.config().sha256,
                    diff_ids: verified.diff_ids().to_vec(),
                    architecture: verified.architecture().to_owned(),
                    compressed_bytes: verified.layers()[0].bytes,
                    unpacked_bytes: verified.unpacked_bytes(),
                    artifact_sha256: digest.finalize().into(),
                    artifact_size: size,
                    created_at_ms: open_compute_core::wall_time_ms(),
                },
                output,
            ))
        })
        .await
        .map_err(|_| unavailable())??;
        #[cfg(feature = "test-support")]
        diagnostic_phase("snapshot-archive-hashed");
        repo.stage_snapshot_artifact(self.instance_id(), id, &source, &image)?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("snapshot-artifact-staged");
        let expected = image_artifact(&image)?;
        let published = self
            .artifacts
            .put_verified_open_file(output, &expected.sha256_hex(), expected.size(), || {
                repo.mark_snapshot_put_attempted(self.instance_id(), id, &source)
            })
            .await?;
        #[cfg(feature = "test-support")]
        diagnostic_phase("snapshot-publication-returned");
        if published != expected {
            return Err(unavailable());
        }
        // Capture files are closed; image materialization reserves its own transport space.
        drop(capture_disk);
        self.ensure_active().await?;
        let size = if request.directory.is_some() {
            captured_bytes
        } else {
            self.materialize_image(&image.manifest_sha256).await?
        };
        #[cfg(feature = "test-support")]
        diagnostic_phase("snapshot-image-materialized");
        let snapshot = repo.finish_snapshot_capture(
            &authority,
            id,
            &source,
            size,
            open_compute_core::wall_time_ms(),
        )?;
        drop(guard);
        Ok(SnapshotResponse {
            snapshot: SnapshotMetadata {
                id: snapshot.id,
                size: snapshot.image_size.ok_or_else(unavailable)?,
                name: snapshot.name,
                dir: snapshot.directory,
            },
        })
    }
}

fn image_artifact(image: &ContainerImage) -> Result<ArtifactRef, PlatformError> {
    ArtifactRef::new(
        open_compute_artifacts::ARTIFACT_KEY_VERSION,
        &hex::encode(image.artifact_sha256),
        image.artifact_size,
    )
}

fn image_limits(image: &ContainerImage) -> ContainerImageLimits {
    ContainerImageLimits {
        json_bytes: 1 << 20,
        compressed_bytes: image.compressed_bytes,
        unpacked_bytes: image.unpacked_bytes,
        layers: 128,
        entries: 100_000,
    }
}
