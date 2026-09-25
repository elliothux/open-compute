//! Native Worker Loader namespace identity owned by immutable binding authority.

use open_compute_core::{InstanceId, PlatformError, VersionId, WorkerId};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// A native loader capability derived from verified Script and binding authority.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkerLoaderBinding {
    /// Declared tenant environment name.
    pub name: String,
    /// Private Script and route-generation namespace key; never placed in tenant env.
    pub namespace_key: String,
}

impl std::fmt::Debug for RuntimeWorkerLoaderBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeWorkerLoaderBinding")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Derive the prefix shared by every native namespace owned by one Script.
#[must_use]
pub fn worker_loader_namespace_prefix(instance_id: InstanceId, worker_id: WorkerId) -> String {
    let mut scope = Sha256::new();
    scope.update(b"oc/worker-loader-scope/v1");
    scope.update(instance_id.as_uuid().as_bytes());
    scope.update(worker_id.as_uuid().as_bytes());
    format!("{}/", hex::encode(scope.finalize()))
}

/// Derive the prefix shared by one Script deployment generation.
#[must_use]
pub fn worker_loader_generation_prefix(
    instance_id: InstanceId,
    worker_id: WorkerId,
    route_generation: u64,
) -> String {
    format!(
        "{}{:016x}/",
        worker_loader_namespace_prefix(instance_id, worker_id),
        route_generation
    )
}

/// Derive a private native namespace from an authorized Script generation and binding name.
///
/// Callers must obtain all inputs from verified binding authority. This digest is an identity,
/// not a bearer credential; only the trusted host's native factory can create the capability.
#[must_use]
pub fn worker_loader_namespace_key(
    instance_id: InstanceId,
    worker_id: WorkerId,
    route_generation: u64,
    binding_name: &str,
) -> String {
    let mut binding = Sha256::new();
    binding.update(b"oc/worker-loader-binding/v1");
    binding.update(binding_name.as_bytes());
    format!(
        "{}{}",
        worker_loader_generation_prefix(instance_id, worker_id, route_generation),
        hex::encode(binding.finalize())
    )
}

/// Whether a persisted Version declares a native Loader binding.
pub fn version_has_worker_loader(
    db: &open_compute_storage::ControlDb,
    version_id: VersionId,
) -> Result<bool, PlatformError> {
    let (_, bindings) = open_compute_storage::version_runtime_features(db, version_id)?;
    Ok(bindings
        .iter()
        .any(|binding| binding.kind == open_compute_storage::BuiltinBindingKind::WorkerLoader))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_follow_script_and_binding_identity() {
        let instance = InstanceId::generate();
        let worker = WorkerId::generate();
        let original = worker_loader_namespace_key(instance, worker, 1, "LOADER");
        assert_eq!(original.len(), 146);
        assert!(original.starts_with(&worker_loader_generation_prefix(instance, worker, 1)));
        assert_eq!(
            original,
            worker_loader_namespace_key(instance, worker, 1, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(InstanceId::generate(), worker, 1, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(instance, WorkerId::generate(), 1, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(instance, worker, 2, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(instance, worker, 1, "OTHER")
        );
    }
}
