//! Authenticated immutable-object pins frozen for one daemon ownership window.

use crate::backup_cli::load_manifest;
use crate::config_load::LoadedConfig;
use open_compute_artifacts::{
    ARTIFACT_KEY_VERSION, ArtifactRef, S3ArtifactClient, SnapshotObjectStore,
};
use open_compute_core::{ErrorCode, PlatformError, PlatformId};
use open_compute_storage::inspect_master_key;
use std::collections::HashSet;

/// Snapshot references loaded once while the daemon owns the data directory.
#[derive(Clone, Debug)]
pub(crate) enum SnapshotPins {
    /// Every committed manifest was authenticated and its pins were parsed.
    Verified {
        artifact_refs: HashSet<ArtifactRef>,
        object_keys: HashSet<String>,
    },
    /// Listing, authentication, or parsing failed; physical GC must remain disabled.
    Unavailable,
}

impl SnapshotPins {
    /// Empty verified set used before production startup wiring or in isolated tests.
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self::Verified {
            artifact_refs: HashSet::new(),
            object_keys: HashSet::new(),
        }
    }

    /// Add authenticated Worker bundle pins to the current live-reference set.
    pub(crate) fn extend_artifacts(
        &self,
        retained: &mut HashSet<ArtifactRef>,
    ) -> Result<(), PlatformError> {
        match self {
            Self::Verified { artifact_refs, .. } => {
                retained.extend(artifact_refs.iter().cloned());
                Ok(())
            }
            Self::Unavailable => Err(pins_unavailable()),
        }
    }

    /// Refuse exact immutable-object deletion while any authenticated manifest pins it.
    pub(crate) fn ensure_unpinned(&self, key: &str) -> Result<(), PlatformError> {
        match self {
            Self::Verified { object_keys, .. } if !object_keys.contains(key) => Ok(()),
            Self::Verified { .. } => Err(PlatformError::new(
                ErrorCode::ResourceReferenced,
                "immutable object is pinned by a committed platform snapshot",
            )),
            Self::Unavailable => Err(pins_unavailable()),
        }
    }
}

/// Load and authenticate all committed manifests for the stable daemon ownership window.
pub(crate) async fn load_snapshot_pins(
    loaded: &LoadedConfig,
    platform_id: PlatformId,
    client: S3ArtifactClient,
) -> Result<SnapshotPins, PlatformError> {
    let key = inspect_master_key(&loaded.config.storage)?;
    let objects = SnapshotObjectStore::new(client, platform_id);
    let mut artifact_refs = HashSet::new();
    let mut object_keys = HashSet::new();
    for snapshot in objects.list_committed().await? {
        let manifest = load_manifest(loaded, &objects, &snapshot.snapshot_id, &key).await?;
        for reference in manifest.immutable_references {
            object_keys.insert(reference.object_key);
            if reference.role == "worker_bundle" {
                artifact_refs.insert(ArtifactRef::new(
                    ARTIFACT_KEY_VERSION,
                    &reference.sha256,
                    reference.size,
                )?);
            }
        }
    }
    Ok(SnapshotPins::Verified {
        artifact_refs,
        object_keys,
    })
}

fn pins_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceReferenced,
        "snapshot pin inventory is unavailable; immutable object GC is disabled",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_pins_are_exact_and_unavailable_inventory_disables_gc() {
        let artifact = ArtifactRef::new(1, &"01".repeat(32), 7).unwrap();
        let pins = SnapshotPins::Verified {
            artifact_refs: HashSet::from([artifact.clone()]),
            object_keys: HashSet::from(["system/backups/kv/owned/data.sqlite".to_owned()]),
        };
        let mut retained = HashSet::new();
        pins.extend_artifacts(&mut retained).unwrap();
        assert_eq!(retained, HashSet::from([artifact]));
        assert!(
            pins.ensure_unpinned("system/backups/kv/other/data.sqlite")
                .is_ok()
        );
        assert_eq!(
            pins.ensure_unpinned("system/backups/kv/owned/data.sqlite")
                .unwrap_err()
                .code(),
            ErrorCode::ResourceReferenced
        );
        assert_eq!(
            SnapshotPins::Unavailable
                .extend_artifacts(&mut retained)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceReferenced
        );
    }
}
