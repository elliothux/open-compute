//! Materialize only the frozen, retained directory inputs before native startup.

use super::*;
use open_compute_artifacts::{ArtifactRef, ContainerImageLimits};
use open_compute_storage::containers::{ContainerDirectoryRestore, ContainerImage, ContainerLease};
use std::{io::Seek as _, os::unix::fs::PermissionsExt as _};

impl ContainerService {
    fn current_directory_inputs(
        &self,
        authority: &ContainerObjectAuthority,
        lease: &ContainerLease,
    ) -> Result<Vec<(ContainerDirectoryRestore, Option<ContainerImage>)>, PlatformError> {
        let inputs = ContainerAuthority::new(self.storage.db())
            .directory_restore_inputs(authority, &lease.opaque_engine_key)?;
        if inputs
            .iter()
            .map(|(entry, _)| entry)
            .ne(lease.directory_snapshots.iter())
        {
            return Err(unavailable());
        }
        Ok(inputs)
    }

    pub(super) async fn prepare_directories(
        &self,
        authority: &ContainerObjectAuthority,
        lease: &ContainerLease,
        registration: &BrokerLease,
    ) -> Result<(), PlatformError> {
        if lease.directory_snapshots.is_empty() {
            return Ok(());
        }
        let inputs = self.current_directory_inputs(authority, lease)?;
        let observed = self.provider.observe(&lease.opaque_engine_key).await?;
        if observed.image != registration.image
            || observed.manifest_sha256 != registration.manifest_sha256
            || observed.provider_contract_sha256 != registration.provider_contract_sha256
            || observed.late_operations_fenced
            || observed.cleanup_complete
            || observed.terminated
            || (observed.running && !observed.directories_prepared)
        {
            return Err(unavailable());
        }
        if !observed.directories_prepared {
            for (position, (_, backup)) in inputs.iter().enumerate() {
                let Some(image) = backup else { continue };
                if self.admitted_image(&image.manifest_sha256)? != *image {
                    return Err(unavailable());
                }
                let image = image.clone();
                let disk_bytes = image
                    .artifact_size
                    .checked_add(image.unpacked_bytes)
                    .ok_or_else(unavailable)?;
                let _reservation = self.storage.reserve_mutation(disk_bytes)?;
                let artifact = ArtifactRef::new(
                    open_compute_artifacts::ARTIFACT_KEY_VERSION,
                    &hex::encode(image.artifact_sha256),
                    image.artifact_size,
                )?;
                let pin = self.cache.acquire(&self.artifacts, &artifact).await?;
                let temporary = self.storage.data_dir().prepare_tmp_dir()?;
                let output = tokio::task::spawn_blocking(move || {
                    let limits = ContainerImageLimits {
                        json_bytes: 1 << 20,
                        compressed_bytes: image.compressed_bytes,
                        unpacked_bytes: image.unpacked_bytes,
                        layers: 1,
                        entries: 100_000,
                    };
                    let output = tempfile::tempfile_in(temporary).map_err(|_| unavailable())?;
                    output
                        .set_permissions(std::fs::Permissions::from_mode(0o600))
                        .map_err(|_| unavailable())?;
                    let output = open_compute_artifacts::write_container_directory_archive(
                        pin.file(),
                        &artifact,
                        image.manifest_sha256,
                        &image.architecture,
                        limits,
                        std::io::BufWriter::with_capacity(256 * 1024, output),
                    )?;
                    let mut output = output.into_inner().map_err(|_| unavailable())?;
                    if output.metadata().map_err(|_| unavailable())?.len() != image.unpacked_bytes {
                        return Err(unavailable());
                    }
                    output.rewind().map_err(|_| unavailable())?;
                    Ok::<_, PlatformError>(output)
                })
                .await
                .map_err(|_| unavailable())??;
                if self.current_directory_inputs(authority, lease)? != inputs {
                    return Err(unavailable());
                }
                self.provider
                    .stage_directory_restore(registration, position, output)
                    .await?;
            }
        }
        if self.current_directory_inputs(authority, lease)? != inputs {
            return Err(unavailable());
        }
        Ok(())
    }
}
