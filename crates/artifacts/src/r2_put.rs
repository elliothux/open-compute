//! Conditional single-part R2 PUT over the backend-neutral object contract.

use super::{
    R2ObjectStore, apply_checksum_metadata, customer_key, integrity_error, map_mutation,
    object_key, object_user_metadata, provider_unavailable, validate_upload,
};
use crate::backend::open_private_source;
use crate::backend::{
    ObjectHttpMetadata, ObjectMetadata, ObjectSource, ObjectStorageClass, PutMode, PutOptions,
};
use crate::r2_model::{
    R2BucketLocator, R2ObjectMetadata, R2PutOptions, R2SsecKey, R2UploadSource, UserObjectKey,
};
use open_compute_core::PlatformError;

const PUT_CONDITION_ATTEMPTS: u8 = 3;

impl R2ObjectStore {
    /// Upload one already-staged, replayable single-part object and verify it by HEAD.
    pub async fn put_file(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        source: &R2UploadSource,
        options: &R2PutOptions,
        current_ssec: Option<&R2SsecKey>,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        validate_upload(source, options)?;
        for attempt in 0..PUT_CONDITION_ATTEMPTS {
            let Some(mode) = self.put_mode(locator, key, options, current_ssec).await? else {
                return Ok(None);
            };
            let file = open_private_source(&source.path, source.length)
                .map_err(|_| provider_unavailable())?;
            let mut user = object_user_metadata(source, options)?;
            apply_checksum_metadata(&mut user, &source.checksums, options.checksum.as_ref());
            let result = self
                .backend
                .put(
                    &object_key(&self.object_key(locator, key))?,
                    ObjectSource::File {
                        file,
                        length: source.length,
                    },
                    PutOptions {
                        mode,
                        metadata: ObjectMetadata {
                            user,
                            http: ObjectHttpMetadata {
                                content_type: options.http_metadata.content_type.clone(),
                                content_language: options.http_metadata.content_language.clone(),
                                content_disposition: options
                                    .http_metadata
                                    .content_disposition
                                    .clone(),
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
                        customer_key: customer_key(options.ssec.as_ref()),
                    },
                )
                .await;
            match result {
                Ok(_) => return self.verify_put(locator, key, source, options).await,
                Err(crate::BackendError::PreconditionFailed)
                    if attempt + 1 < PUT_CONDITION_ATTEMPTS => {}
                Err(crate::BackendError::PreconditionFailed) => {
                    return self.final_precondition(locator, key, options).await;
                }
                Err(failure) => return Err(map_mutation(failure)),
            }
        }
        Err(provider_unavailable())
    }

    async fn put_mode(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        options: &R2PutOptions,
        current_ssec: Option<&R2SsecKey>,
    ) -> Result<Option<PutMode>, PlatformError> {
        let Some(condition) = &options.only_if else {
            return Ok(Some(PutMode::Replace));
        };
        match self.head(locator, key, current_ssec).await? {
            None if condition.matches_missing() => Ok(Some(PutMode::CreateOnly)),
            None => Ok(None),
            Some(metadata) if condition.matches_object(&metadata.etag, metadata.uploaded) => {
                Ok(Some(PutMode::IfMatch(metadata.etag)))
            }
            Some(_) => Ok(None),
        }
    }

    async fn verify_put(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        source: &R2UploadSource,
        options: &R2PutOptions,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let metadata = self
            .head(locator, key, options.ssec.as_ref())
            .await?
            .ok_or_else(integrity_error)?;
        if metadata.size != source.length
            || metadata.version != source.version
            || metadata.checksums != source.checksums.exposed(options.checksum.as_ref())
            || metadata.storage_class != options.storage_class.as_str()
            || metadata.ssec_key_md5.as_deref()
                != options.ssec.as_ref().map(R2SsecKey::md5_hex).as_deref()
        {
            return Err(integrity_error());
        }
        Ok(Some(metadata))
    }

    async fn final_precondition(
        &self,
        locator: &R2BucketLocator,
        key: &UserObjectKey,
        options: &R2PutOptions,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        let Some(condition) = &options.only_if else {
            return Err(provider_unavailable());
        };
        let current = self.head(locator, key, options.ssec.as_ref()).await?;
        match current {
            None if condition.matches_missing() => Err(provider_unavailable()),
            Some(metadata) if condition.matches_object(&metadata.etag, metadata.uploaded) => {
                Err(provider_unavailable())
            }
            _ => Ok(None),
        }
    }
}
