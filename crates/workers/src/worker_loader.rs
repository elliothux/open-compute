//! Native Worker Loader namespace identity owned by immutable binding authority.

use open_compute_core::{AccountId, WorkerId};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// A native loader capability derived from verified Script and binding authority.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkerLoaderBinding {
    /// Declared tenant environment name.
    pub name: String,
    /// Private Script-scoped native namespace key; never placed in tenant env.
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

/// Derive a private native namespace from an authorized Script and canonical binding name.
///
/// Callers must obtain all inputs from verified binding authority. This digest is an identity,
/// not a bearer credential; only the trusted host's native factory can create the capability.
#[must_use]
pub fn worker_loader_namespace_key(
    account_id: AccountId,
    worker_id: WorkerId,
    binding_name: &str,
) -> String {
    let mut namespace = Sha256::new();
    namespace.update(b"oc/public-worker-loader/v1");
    // Fixed-width UUID bytes make the concatenation unambiguous without a private wire format.
    namespace.update(account_id.as_uuid().as_bytes());
    namespace.update(worker_id.as_uuid().as_bytes());
    namespace.update(binding_name.as_bytes());
    hex::encode(namespace.finalize())
}

/// Collect all namespaces owned by a Script, including retained tombstoned Versions.
///
/// Namespaces survive Version deletion, so Script cleanup must include declarations from every
/// retained Version rather than only the active deployment's bindings.
pub fn worker_loader_namespaces(
    db: &open_compute_storage::ControlDb,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<Vec<String>, open_compute_core::PlatformError> {
    let versions =
        open_compute_storage::WorkerRepository::new(db).list_versions(account_id, worker_id)?;
    let mut keys = std::collections::BTreeSet::new();
    for version in versions {
        let (_, bindings) = open_compute_storage::version_runtime_features(db, version.id)?;
        for binding in bindings {
            if binding.kind == open_compute_storage::BuiltinBindingKind::WorkerLoader {
                crate::validate_env_name(&binding.name)?;
                keys.insert(worker_loader_namespace_key(
                    account_id,
                    worker_id,
                    &binding.name,
                ));
            }
        }
    }
    Ok(keys.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_follow_script_and_binding_identity() {
        let account = AccountId::generate();
        let worker = WorkerId::generate();
        let original = worker_loader_namespace_key(account, worker, "LOADER");
        assert_eq!(original.len(), 64);
        assert_eq!(
            original,
            worker_loader_namespace_key(account, worker, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(AccountId::generate(), worker, "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(account, WorkerId::generate(), "LOADER")
        );
        assert_ne!(
            original,
            worker_loader_namespace_key(account, worker, "OTHER")
        );
    }
}
