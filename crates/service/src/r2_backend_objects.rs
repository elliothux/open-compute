//! Durable object-metadata authority for R2 provider operations.

use super::*;
use open_compute_artifacts::{R2ObjectMetadata, R2SsecKey, UserObjectKey};
use open_compute_core::SecretBytes;
use open_compute_storage::{
    R2ObjectMutationKind, R2ObjectMutationRecord, R2ObjectRecord, R2ObjectRepository,
    SecretEnvelope,
};

impl R2BindingService {
    pub(super) async fn authoritative_head(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        key: &UserObjectKey,
        timeout: Duration,
    ) -> Result<Option<R2ObjectMetadata>, PlatformError> {
        self.ensure_no_object_mutation(binding, key)?;
        let Some(record) = R2ObjectRepository::new(self.storage.db()).get(
            binding.account_id,
            binding.resource.id,
            key.as_str(),
        )?
        else {
            self.require_provider_absent(locator, key, timeout).await?;
            return Ok(None);
        };
        let ssec = open_object_ssec(&self.storage, &record)?;
        let metadata = timeout_result(timeout, self.objects.head(locator, key, ssec.as_ref()))
            .await?
            .ok_or_else(metadata_invalid)?;
        validate_object_record(&record, &metadata)?;
        Ok(Some(metadata))
    }

    pub(super) async fn committed_object(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        key: &UserObjectKey,
        timeout: Duration,
    ) -> Result<Option<(R2ObjectRecord, Option<R2SsecKey>)>, PlatformError> {
        self.ensure_no_object_mutation(binding, key)?;
        let record = R2ObjectRepository::new(self.storage.db()).get(
            binding.account_id,
            binding.resource.id,
            key.as_str(),
        )?;
        if record.is_none() {
            self.require_provider_absent(locator, key, timeout).await?;
        }
        record
            .map(|record| {
                let ssec = open_object_ssec(&self.storage, &record)?;
                Ok((record, ssec))
            })
            .transpose()
    }

    pub(super) fn ensure_no_object_mutation(
        &self,
        binding: &AuthorizedBinding,
        key: &UserObjectKey,
    ) -> Result<(), PlatformError> {
        if R2ObjectRepository::new(self.storage.db())
            .get_mutation(binding.account_id, binding.resource.id, key.as_str())?
            .is_some()
        {
            return Err(PlatformError::new(
                ErrorCode::R2ProviderUnavailable,
                "R2 object mutation is still being resolved",
            ));
        }
        Ok(())
    }

    async fn require_provider_absent(
        &self,
        locator: &open_compute_artifacts::R2BucketLocator,
        key: &UserObjectKey,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        if timeout_result(timeout, self.objects.head(locator, key, None))
            .await?
            .is_some()
        {
            return Err(metadata_invalid());
        }
        Ok(())
    }

    pub(super) fn begin_object_put(
        &self,
        binding: &AuthorizedBinding,
        key: &UserObjectKey,
        version: &str,
        ssec: Option<&R2SsecKey>,
    ) -> Result<(), PlatformError> {
        let (ssec_key_md5, ssec_envelope) =
            seal_object_ssec(&self.storage, binding, version, ssec)?;
        R2ObjectRepository::new(self.storage.db()).begin_put(
            &R2ObjectRecord {
                resource_id: binding.resource.id,
                account_id: binding.account_id,
                object_key: key.as_str().to_owned(),
                object_version: version.to_owned(),
                ssec_key_md5,
                ssec_envelope,
            },
            i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
        )
    }

    pub(super) fn finish_object_put(
        &self,
        binding: &AuthorizedBinding,
        key: &UserObjectKey,
        metadata: &R2ObjectMetadata,
    ) -> Result<(), PlatformError> {
        if metadata.key != key.as_str() {
            return Err(metadata_invalid());
        }
        let repo = R2ObjectRepository::new(self.storage.db());
        let record = if repo
            .get_mutation(binding.account_id, binding.resource.id, key.as_str())?
            .is_some()
        {
            repo.finish_put(
                binding.account_id,
                binding.resource.id,
                key.as_str(),
                &metadata.version,
                i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
            )?
        } else {
            repo.get(binding.account_id, binding.resource.id, key.as_str())?
                .ok_or_else(metadata_invalid)?
        };
        validate_object_record(&record, metadata)
    }

    pub(super) async fn reconcile_object_key(
        &self,
        binding: &AuthorizedBinding,
        locator: &open_compute_artifacts::R2BucketLocator,
        key: &UserObjectKey,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        let repo = R2ObjectRepository::new(self.storage.db());
        let Some(mutation) =
            repo.get_mutation(binding.account_id, binding.resource.id, key.as_str())?
        else {
            return Ok(());
        };
        self.reconcile_object_mutation(locator, &mutation, timeout)
            .await
    }

    pub(super) async fn reconcile_object_mutation(
        &self,
        locator: &open_compute_artifacts::R2BucketLocator,
        mutation: &R2ObjectMutationRecord,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        let repo = R2ObjectRepository::new(self.storage.db());
        let key = UserObjectKey::parse(&mutation.object_key)?;
        let committed = repo.get(
            mutation.account_id,
            mutation.resource_id,
            &mutation.object_key,
        )?;
        match mutation.kind {
            R2ObjectMutationKind::Put => {
                let pending_ssec = open_mutation_ssec(&self.storage, mutation)?;
                let pending = timeout_result(
                    timeout,
                    self.objects.head(locator, &key, pending_ssec.as_ref()),
                )
                .await;
                match pending {
                    Ok(Some(metadata))
                        if mutation.pending_version.as_deref()
                            == Some(metadata.version.as_str()) =>
                    {
                        let record = repo.finish_put(
                            mutation.account_id,
                            mutation.resource_id,
                            &mutation.object_key,
                            &metadata.version,
                            i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
                        )?;
                        validate_object_record(&record, &metadata)
                    }
                    Ok(Some(metadata)) => {
                        reconcile_committed_or_fail(
                            self,
                            locator,
                            repo,
                            mutation,
                            &key,
                            CommittedObservation {
                                committed: committed.as_ref(),
                                observed_with_pending_key: Some(metadata),
                                timeout,
                            },
                        )
                        .await
                    }
                    Ok(None) => {
                        reconcile_committed_or_fail(
                            self,
                            locator,
                            repo,
                            mutation,
                            &key,
                            CommittedObservation {
                                committed: committed.as_ref(),
                                observed_with_pending_key: None,
                                timeout,
                            },
                        )
                        .await
                    }
                    Err(error) if error.code() == ErrorCode::R2SsecInvalid => {
                        reconcile_committed_or_fail(
                            self,
                            locator,
                            repo,
                            mutation,
                            &key,
                            CommittedObservation {
                                committed: committed.as_ref(),
                                observed_with_pending_key: None,
                                timeout,
                            },
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            }
            R2ObjectMutationKind::Delete => {
                let committed = committed.ok_or_else(metadata_invalid)?;
                let ssec = open_object_ssec(&self.storage, &committed)?;
                match timeout_result(timeout, self.objects.head(locator, &key, ssec.as_ref()))
                    .await?
                {
                    None => repo.finish_delete(
                        mutation.account_id,
                        mutation.resource_id,
                        std::slice::from_ref(&mutation.object_key),
                    ),
                    Some(metadata) => {
                        validate_object_record(&committed, &metadata)?;
                        repo.cancel_delete(
                            mutation.account_id,
                            mutation.resource_id,
                            &mutation.object_key,
                        )
                    }
                }
            }
        }
    }
}

struct CommittedObservation<'a> {
    committed: Option<&'a R2ObjectRecord>,
    observed_with_pending_key: Option<R2ObjectMetadata>,
    timeout: Duration,
}

async fn reconcile_committed_or_fail(
    service: &R2BindingService,
    locator: &open_compute_artifacts::R2BucketLocator,
    repo: R2ObjectRepository<'_>,
    mutation: &R2ObjectMutationRecord,
    key: &UserObjectKey,
    observation: CommittedObservation<'_>,
) -> Result<(), PlatformError> {
    let CommittedObservation {
        committed,
        observed_with_pending_key,
        timeout,
    } = observation;
    let Some(committed) = committed else {
        if observed_with_pending_key.is_some() {
            return Err(metadata_invalid());
        }
        return repo.cancel_put(
            mutation.account_id,
            mutation.resource_id,
            &mutation.object_key,
        );
    };
    if let Some(metadata) = observed_with_pending_key
        && metadata.version == committed.object_version
    {
        validate_object_record(committed, &metadata)?;
        return repo.cancel_put(
            mutation.account_id,
            mutation.resource_id,
            &mutation.object_key,
        );
    }
    let ssec = open_object_ssec(&service.storage, committed)?;
    let metadata = timeout_result(timeout, service.objects.head(locator, key, ssec.as_ref()))
        .await?
        .ok_or_else(metadata_invalid)?;
    validate_object_record(committed, &metadata)?;
    repo.cancel_put(
        mutation.account_id,
        mutation.resource_id,
        &mutation.object_key,
    )
}

pub(crate) async fn reconcile_bucket_objects(
    storage: &Arc<PlatformStorage>,
    objects: &R2ObjectStore,
    bucket: &open_compute_storage::R2BucketRecord,
    timeout: Duration,
) -> Result<u64, PlatformError> {
    let service = R2BindingService::new(
        storage.clone(),
        ResourcePins::new(),
        objects.clone(),
        R2Config::default(),
    )?;
    let locator = objects.locator(bucket.resource.id, &bucket.physical_prefix)?;
    let mutations = R2ObjectRepository::new(storage.db()).list_mutations(bucket.resource.id)?;
    let mut reconciled = 0_u64;
    for mutation in mutations {
        service
            .reconcile_object_mutation(&locator, &mutation, timeout)
            .await?;
        reconciled = reconciled.saturating_add(1);
    }
    Ok(reconciled)
}

pub(super) fn seal_object_ssec(
    storage: &PlatformStorage,
    binding: &AuthorizedBinding,
    version: &str,
    ssec: Option<&R2SsecKey>,
) -> Result<(Option<String>, Option<String>), PlatformError> {
    let Some(ssec) = ssec else {
        return Ok((None, None));
    };
    let envelope = storage.crypto().encrypt_r2_object_ssec(
        &SecretBytes::new(ssec.as_bytes().to_vec()),
        binding.account_id,
        binding.resource.id,
        version,
    )?;
    Ok((
        Some(ssec.md5_base64()),
        Some(serde_json::to_string(&envelope).map_err(|_| protocol_error())?),
    ))
}

pub(crate) fn open_object_ssec(
    storage: &PlatformStorage,
    record: &R2ObjectRecord,
) -> Result<Option<R2SsecKey>, PlatformError> {
    open_sealed_ssec(
        storage,
        record.account_id,
        record.resource_id,
        &record.object_version,
        record.ssec_key_md5.as_deref(),
        record.ssec_envelope.as_deref(),
    )
}

fn open_mutation_ssec(
    storage: &PlatformStorage,
    record: &R2ObjectMutationRecord,
) -> Result<Option<R2SsecKey>, PlatformError> {
    open_sealed_ssec(
        storage,
        record.account_id,
        record.resource_id,
        record
            .pending_version
            .as_deref()
            .ok_or_else(protocol_error)?,
        record.pending_ssec_key_md5.as_deref(),
        record.pending_ssec_envelope.as_deref(),
    )
}

fn open_sealed_ssec(
    storage: &PlatformStorage,
    account_id: AccountId,
    resource_id: ResourceId,
    version: &str,
    expected_md5: Option<&str>,
    raw_envelope: Option<&str>,
) -> Result<Option<R2SsecKey>, PlatformError> {
    let Some(raw) = raw_envelope else {
        return if expected_md5.is_none() {
            Ok(None)
        } else {
            Err(metadata_invalid())
        };
    };
    let expected_md5 = expected_md5.ok_or_else(metadata_invalid)?;
    let envelope: SecretEnvelope = serde_json::from_str(raw).map_err(|_| metadata_invalid())?;
    let plaintext =
        storage
            .crypto()
            .decrypt_r2_object_ssec(&envelope, account_id, resource_id, version)?;
    let ssec = R2SsecKey::from_bytes(plaintext.expose())?;
    if ssec.md5_base64() != expected_md5 {
        return Err(metadata_invalid());
    }
    Ok(Some(ssec))
}

pub(super) fn validate_object_record(
    record: &R2ObjectRecord,
    metadata: &R2ObjectMetadata,
) -> Result<(), PlatformError> {
    if metadata.key != record.object_key
        || metadata.version != record.object_version
        || metadata.ssec_key_md5 != record.ssec_key_md5
    {
        return Err(metadata_invalid());
    }
    Ok(())
}
