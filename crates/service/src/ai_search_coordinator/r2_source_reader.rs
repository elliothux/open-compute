//! Frozen R2 revision reader for AI Search indexing.

use super::*;
use open_compute_artifacts::{R2Condition, R2EtagMatch, R2GetResult, R2ObjectStore, UserObjectKey};
use open_compute_storage::{PlatformStorage, R2BucketRepository, R2ObjectRepository};

/// Production reader that dispatches built-in and frozen R2 source locators.
#[derive(Clone, Debug)]
pub struct PlatformAiSearchSourceReader {
    builtin: ObjectAiSearchSourceReader,
    storage: Arc<PlatformStorage>,
    r2_objects: R2ObjectStore,
    r2_bucket: ResourceId,
    maximum_bytes: u64,
    operation_timeout: Duration,
}

impl PlatformAiSearchSourceReader {
    /// Bind one R2-backed instance to its immutable logical bucket identity.
    #[must_use]
    pub fn new(
        builtin: ObjectAiSearchSourceReader,
        storage: Arc<PlatformStorage>,
        r2_objects: R2ObjectStore,
        r2_bucket: ResourceId,
        maximum_bytes: u64,
        operation_timeout: Duration,
    ) -> Self {
        Self {
            builtin,
            storage,
            r2_objects,
            r2_bucket,
            maximum_bytes,
            operation_timeout,
        }
    }
}

impl AiSearchSourceReader for PlatformAiSearchSourceReader {
    fn read<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async move {
            let AiSearchSourceReference::R2(source) = &claim.item.source else {
                return self.builtin.read(claim).await;
            };
            if source.object_size == 0 || source.object_size > self.maximum_bytes {
                return Err(limit());
            }
            let account = self.builtin.account;
            let key = UserObjectKey::parse(&claim.item.key)?;
            let repository = R2ObjectRepository::new(self.storage.db());
            if repository
                .get_mutation(account, self.r2_bucket, key.as_str())?
                .is_some()
            {
                return Err(unavailable());
            }
            let record = repository
                .get(account, self.r2_bucket, key.as_str())?
                .ok_or_else(unavailable)?;
            if record.object_version != source.object_version {
                return Err(unavailable());
            }
            let bucket = R2BucketRepository::new(self.storage.db()).get(account, self.r2_bucket)?;
            let locator = self
                .r2_objects
                .locator(self.r2_bucket, &bucket.physical_prefix)?;
            let ssec = crate::r2_backend::objects::open_object_ssec(&self.storage, &record)?;
            let condition = R2Condition {
                etag_matches: vec![R2EtagMatch::Strong {
                    value: source.etag.clone(),
                }],
                ..R2Condition::default()
            };
            let download = match tokio::time::timeout(
                self.operation_timeout,
                self.r2_objects
                    .get(&locator, &key, None, Some(&condition), ssec.as_ref()),
            )
            .await
            .map_err(|_| unavailable())??
            {
                R2GetResult::Body(download) => download,
                R2GetResult::Missing | R2GetResult::Precondition(_) => return Err(unavailable()),
            };
            crate::r2_backend::objects::validate_object_record(&record, &download.metadata)?;
            if download.metadata.etag != source.etag
                || download.metadata.size != source.object_size
                || download.metadata.uploaded != source.uploaded_at_ms
            {
                return Err(integrity());
            }
            let bytes = download
                .body
                .collect()
                .await
                .map_err(|_| unavailable())?
                .into_bytes();
            if u64::try_from(bytes.len()).map_err(|_| limit())? != source.object_size {
                return Err(integrity());
            }
            Ok(AiSearchSourceDocument {
                bytes: bytes.to_vec(),
            })
        })
    }
}
