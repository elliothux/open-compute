//! Composition of the configured object-byte authority.

use open_compute_artifacts::{
    ObjectBackend, R2_MAX_MULTIPART_OBJECT_BYTES, S3Credentials, SnapshotObjectStore,
    resolve_s3_credentials, verify_object_authority,
};
use open_compute_core::{
    ErrorCode, ObjectStorageConfig, ObjectStorageKind, PlatformConfig, PlatformError, PlatformId,
};
use open_compute_storage::StableIdentity;

/// Connected backend plus short-lived S3 credentials for redactor registration.
pub(crate) struct ConnectedObjectBackend {
    pub(crate) backend: ObjectBackend,
    pub(crate) credentials: Option<S3Credentials>,
}

pub(crate) async fn discover_snapshot_backend(
    config: &PlatformConfig,
    snapshot_id: &str,
) -> Result<(ObjectBackend, PlatformId), PlatformError> {
    match &config.object_storage {
        ObjectStorageConfig::Local(local) => {
            let (backend, platform_id) =
                ObjectBackend::open_local_existing(local, R2_MAX_MULTIPART_OBJECT_BYTES)?;
            verify_object_authority(&backend, platform_id).await?;
            Ok((backend, platform_id))
        }
        ObjectStorageConfig::S3(s3) => {
            let credentials = resolve_s3_credentials(s3)?;
            let temporary =
                ObjectBackend::connect_s3(s3, &credentials, R2_MAX_MULTIPART_OBJECT_BYTES)?;
            let discovered = SnapshotObjectStore::discover(temporary, snapshot_id).await?;
            let platform_id = discovered.platform_id();
            let backend =
                ObjectBackend::connect_s3(s3, &credentials, R2_MAX_MULTIPART_OBJECT_BYTES)?;
            verify_object_authority(&backend, platform_id).await?;
            Ok((backend, platform_id))
        }
    }
}

pub(crate) fn connect_object_backend(
    config: &PlatformConfig,
    identity: &StableIdentity,
) -> Result<ConnectedObjectBackend, PlatformError> {
    let expected = match (
        identity.object_backend_kind,
        identity.object_authority_sha256,
    ) {
        (None, None) => None,
        (Some(kind), Some(authority)) => Some((kind, authority)),
        _ => return Err(authority_binding_invalid()),
    };
    if expected.is_some_and(|(kind, _)| kind != config.object_storage.kind()) {
        return Err(authority_mismatch());
    }
    match &config.object_storage {
        ObjectStorageConfig::Local(local) => {
            let backend = if let Some((ObjectStorageKind::Local, authority)) = expected {
                let (backend, platform_id) =
                    ObjectBackend::open_local_existing(local, R2_MAX_MULTIPART_OBJECT_BYTES)?;
                if platform_id != identity.platform_id || backend.authority_sha256() != authority {
                    return Err(authority_mismatch());
                }
                backend
            } else {
                ObjectBackend::open_local(
                    local,
                    identity.platform_id,
                    R2_MAX_MULTIPART_OBJECT_BYTES,
                )?
            };
            Ok(ConnectedObjectBackend {
                backend,
                credentials: None,
            })
        }
        ObjectStorageConfig::S3(s3) => {
            let credentials = resolve_s3_credentials(s3)?;
            let backend =
                ObjectBackend::connect_s3(s3, &credentials, R2_MAX_MULTIPART_OBJECT_BYTES)?;
            if expected.is_some_and(|(kind, authority)| {
                kind != ObjectStorageKind::S3 || authority != backend.authority_sha256()
            }) {
                return Err(authority_mismatch());
            }
            Ok(ConnectedObjectBackend {
                backend,
                credentials: Some(credentials),
            })
        }
    }
}

fn authority_mismatch() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageAuthorityMismatch,
        "object storage authority does not match stored platform identity",
    )
}

fn authority_binding_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageIntegrityError,
        "stored object authority binding is invalid",
    )
}
