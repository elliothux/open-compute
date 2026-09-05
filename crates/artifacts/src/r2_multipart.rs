//! Multipart operations for the typed R2 object store.

use crate::backend::{
    BackendError, ObjectHttpMetadata, ObjectMetadata, ObjectSource, ObjectStorageClass,
    UploadedPart, open_private_source,
};
use crate::r2::{
    R2ObjectStore, create_user_metadata, customer_key, map_backend, map_mutation, object_key,
    provider_unavailable,
};
use crate::r2_codec::integrity_error;
use crate::r2_model::{
    R2BucketLocator, R2MultipartCreateOptions, R2ObjectMetadata, R2SsecKey, R2UploadSource,
    R2UploadedPart, UserObjectKey, invalid_options,
};
use open_compute_core::PlatformError;

impl R2ObjectStore {
    /// List backend multipart ids for one exact tenant key inside its owned bucket prefix.
    pub async fn list_multipart_upload_ids(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
    ) -> Result<Vec<String>, PlatformError> {
        self.backend
            .list_multipart(&object_key(&self.object_key(locator, key))?)
            .await
            .map_err(map_backend)
    }

    /// Start a multipart upload and return its opaque backend upload id.
    pub async fn create_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        version: &str,
        options: &R2MultipartCreateOptions,
    ) -> Result<String, PlatformError> {
        let user = create_user_metadata(
            version,
            &options.custom_metadata,
            options.storage_class,
            options.ssec.as_ref(),
        )?;
        self.backend
            .create_multipart(
                &object_key(&self.object_key(locator, key))?,
                ObjectMetadata {
                    user,
                    http: ObjectHttpMetadata {
                        content_type: options.http_metadata.content_type.clone(),
                        content_language: options.http_metadata.content_language.clone(),
                        content_disposition: options.http_metadata.content_disposition.clone(),
                        content_encoding: options.http_metadata.content_encoding.clone(),
                        cache_control: options.http_metadata.cache_control.clone(),
                        cache_expiry: options.http_metadata.cache_expiry,
                    },
                    storage_class: match options.storage_class {
                        crate::R2StorageClass::Standard => ObjectStorageClass::Standard,
                        crate::R2StorageClass::InfrequentAccess => {
                            ObjectStorageClass::InfrequentAccess
                        }
                    },
                    ..ObjectMetadata::default()
                },
                customer_key(options.ssec.as_ref()),
            )
            .await
            .map_err(map_mutation)
    }

    /// Upload one already-staged part.
    pub async fn upload_part(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
        part_number: i32,
        source: &R2UploadSource,
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2UploadedPart, PlatformError> {
        if !(1..=crate::r2_model::R2_MAX_MULTIPART_PARTS).contains(&part_number)
            || source.length > crate::r2_model::R2_MAX_MULTIPART_PART_BYTES
        {
            return Err(invalid_options());
        }
        let file = open_private_source(&source.path, source.length).map_err(|failure| {
            if failure == BackendError::Unavailable {
                provider_unavailable()
            } else {
                integrity_error()
            }
        })?;
        let part = self
            .backend
            .upload_part(
                &object_key(&self.object_key(locator, key))?,
                provider_upload_id,
                part_number,
                ObjectSource::File {
                    file,
                    length: source.length,
                },
                customer_key(ssec),
            )
            .await
            .map_err(map_multipart_mutation)?;
        Ok(R2UploadedPart {
            part_number: part.part_number,
            etag: part.etag,
        })
    }

    /// Complete a multipart upload and verify the resulting object.
    pub async fn complete_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
        parts: &[R2UploadedPart],
        ssec: Option<&R2SsecKey>,
    ) -> Result<R2ObjectMetadata, PlatformError> {
        if parts.is_empty() {
            return Err(invalid_options());
        }
        let completed = parts
            .iter()
            .map(|part| UploadedPart {
                part_number: part.part_number,
                etag: part.etag.clone(),
            })
            .collect::<Vec<_>>();
        self.backend
            .complete_multipart(
                &object_key(&self.object_key(locator, key))?,
                provider_upload_id,
                &completed,
                customer_key(ssec),
            )
            .await
            .map_err(map_multipart_mutation)?;
        self.head(locator, key, ssec)
            .await?
            .ok_or_else(integrity_error)
    }

    /// Abort a multipart upload. Missing uploads succeed.
    pub async fn abort_multipart_upload(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        provider_upload_id: &str,
    ) -> Result<(), PlatformError> {
        self.backend
            .abort_multipart(
                &object_key(&self.object_key(locator, key))?,
                provider_upload_id,
            )
            .await
            .map_err(map_mutation)
    }
}

fn map_multipart_mutation(failure: BackendError) -> PlatformError {
    match failure {
        BackendError::NotFound | BackendError::MultipartInvalid => {
            crate::r2_model::multipart_invalid()
        }
        other => map_mutation(other),
    }
}
