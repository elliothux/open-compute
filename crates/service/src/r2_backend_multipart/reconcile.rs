//! Startup, maintenance, and deletion reconciliation for durable multipart state.

use super::*;
use std::collections::{BTreeMap, HashSet};

/// Reconcile provider uploads whose create/abort response was lost, and optionally drain every
/// nonterminal upload while deleting a fenced bucket.
pub(crate) async fn reconcile_bucket_multipart(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    bucket: &open_compute_storage::R2BucketRecord,
    classify_startup_initiating: bool,
    drain_all: bool,
    timeout: Duration,
) -> Result<u64, PlatformError> {
    let repo = R2MultipartRepository::new(storage.db());
    let now = i64::try_from(unix_ms()?).map_err(|_| protocol_error())?;
    if classify_startup_initiating {
        repo.mark_resource_initiating_unknown(bucket.resource.id, now)?;
    }
    let locator = objects.locator(bucket.resource.id, &bucket.physical_prefix)?;
    let mut reconciled = 0_u64;
    let rows = repo.list_for_resource(bucket.resource.id)?;
    let known_provider_ids = rows
        .iter()
        .filter_map(|record| record.provider_upload_id.clone())
        .collect::<HashSet<_>>();
    let mut unknown_by_key = BTreeMap::<String, Vec<R2MultipartUploadRecord>>::new();
    for record in &rows {
        if record.state == R2MultipartState::CreateUnknown {
            unknown_by_key
                .entry(record.object_key.clone())
                .or_default()
                .push(record.clone());
        }
    }
    for (object_key, mut unknown) in unknown_by_key {
        if rows.iter().any(|record| {
            record.object_key == object_key && record.state == R2MultipartState::Initiating
        }) {
            continue;
        }
        let key = UserObjectKey::parse(&object_key)?;
        let mut orphan_ids =
            timeout_result(timeout, objects.list_multipart_upload_ids(&locator, &key))
                .await?
                .into_iter()
                .filter(|id| !known_provider_ids.contains(id))
                .collect::<Vec<_>>();
        orphan_ids.sort();
        unknown.sort_by(|left, right| left.upload_id.cmp(&right.upload_id));
        if orphan_ids.is_empty() {
            for record in unknown {
                repo.delete_create_unknown(
                    record.account_id,
                    record.resource_id,
                    &record.upload_id,
                )?;
                reconciled = reconciled.saturating_add(1);
            }
            continue;
        }
        // SDK retries can create more than one provider upload after response loss. Every id in
        // this set is unreferenced, under the exact owned resource/key prefix, and no live
        // initiating row exists. Remove excess provider attempts before assigning durable abort
        // receipts; this never guesses an id for tenant use.
        if orphan_ids.len() > unknown.len() {
            for provider_id in orphan_ids.drain(unknown.len()..) {
                mutation_timeout_result(
                    timeout,
                    objects.abort_multipart_upload(&locator, &key, &provider_id),
                )
                .await?;
                reconciled = reconciled.saturating_add(1);
            }
        }
        let paired = orphan_ids.len();
        for (record, provider_id) in unknown.iter().take(paired).zip(orphan_ids) {
            let claimed = repo.claim_unknown_for_abort(
                record.account_id,
                record.resource_id,
                &record.upload_id,
                &provider_id,
                now,
            )?;
            finish_catalog_abort(objects, &locator, &repo, &claimed, timeout).await?;
            reconciled = reconciled.saturating_add(1);
        }
        for record in unknown.into_iter().skip(paired) {
            repo.delete_create_unknown(record.account_id, record.resource_id, &record.upload_id)?;
            reconciled = reconciled.saturating_add(1);
        }
    }

    let rows = repo.list_for_resource(bucket.resource.id)?;
    for record in rows {
        if record.state == R2MultipartState::Completing && !drain_all {
            reconcile_catalog_complete(storage, objects, &locator, &repo, &record, timeout).await?;
            reconciled = reconciled.saturating_add(1);
            continue;
        }
        let record = match record.state {
            R2MultipartState::Aborting => record,
            R2MultipartState::Initiating
                if classify_startup_initiating && record.provider_upload_id.is_some() =>
            {
                repo.claim_for_cleanup(
                    record.account_id,
                    record.resource_id,
                    &record.upload_id,
                    now,
                )?
            }
            R2MultipartState::Open | R2MultipartState::Completing if drain_all => repo
                .claim_for_cleanup(
                    record.account_id,
                    record.resource_id,
                    &record.upload_id,
                    now,
                )?,
            _ => continue,
        };
        finish_catalog_abort(objects, &locator, &repo, &record, timeout).await?;
        reconciled = reconciled.saturating_add(1);
    }
    Ok(reconciled)
}

async fn finish_catalog_abort(
    objects: &R2ObjectStore,
    locator: &open_compute_artifacts::R2BucketLocator,
    repo: &R2MultipartRepository<'_>,
    record: &R2MultipartUploadRecord,
    timeout: Duration,
) -> Result<(), PlatformError> {
    let provider_upload_id = record
        .provider_upload_id
        .as_deref()
        .ok_or_else(protocol_error)?;
    let key = UserObjectKey::parse(&record.object_key)?;
    mutation_timeout_result(
        timeout,
        objects.abort_multipart_upload(locator, &key, provider_upload_id),
    )
    .await?;
    repo.finish_abort(
        record.account_id,
        record.resource_id,
        &record.upload_id,
        &record.object_key,
        i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
    )?;
    Ok(())
}

async fn reconcile_catalog_complete(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    locator: &open_compute_artifacts::R2BucketLocator,
    repo: &R2MultipartRepository<'_>,
    record: &R2MultipartUploadRecord,
    timeout: Duration,
) -> Result<(), PlatformError> {
    let raw = record
        .completion_manifest
        .as_deref()
        .ok_or_else(protocol_error)?;
    let parts: Vec<open_compute_artifacts::R2UploadedPart> =
        serde_json::from_str(raw).map_err(|_| protocol_error())?;
    if canonical_completion(&parts)? != raw {
        return Err(protocol_error());
    }
    let stored = repo.list_parts(&record.upload_id)?;
    validate_complete_parts(&parts, &stored)?;
    let key = UserObjectKey::parse(&record.object_key)?;
    let ssec = open_ssec(storage, record)?;
    if let Some(metadata) =
        timeout_result(timeout, objects.head(locator, &key, ssec.as_ref())).await?
    {
        return finish_reconciled_complete(storage, *repo, record, &parts, &stored, &metadata);
    }
    let provider_upload_id = record
        .provider_upload_id
        .as_deref()
        .ok_or_else(protocol_error)?;
    let result = mutation_timeout_result(
        timeout,
        objects.complete_multipart_upload(locator, &key, provider_upload_id, &parts, ssec.as_ref()),
    )
    .await;
    match result {
        Ok(metadata) => {
            finish_reconciled_complete(storage, *repo, record, &parts, &stored, &metadata)
        }
        Err(error) if error.code() == ErrorCode::R2ResultUnknown => {
            if let Some(metadata) =
                timeout_result(timeout, objects.head(locator, &key, ssec.as_ref())).await?
            {
                finish_reconciled_complete(storage, *repo, record, &parts, &stored, &metadata)
            } else {
                Err(error)
            }
        }
        Err(_error) => {
            if let Some(metadata) =
                timeout_result(timeout, objects.head(locator, &key, ssec.as_ref())).await?
            {
                return finish_reconciled_complete(
                    storage, *repo, record, &parts, &stored, &metadata,
                );
            }
            repo.revert_complete(
                record.account_id,
                record.resource_id,
                &record.upload_id,
                &record.object_key,
                i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
            )?;
            Ok(())
        }
    }
}

fn finish_reconciled_complete(
    storage: &PlatformStorage,
    repo: R2MultipartRepository<'_>,
    record: &R2MultipartUploadRecord,
    parts: &[open_compute_artifacts::R2UploadedPart],
    stored: &[R2MultipartPartRecord],
    metadata: &R2ObjectMetadata,
) -> Result<(), PlatformError> {
    validate_completed_object(record, parts, stored, metadata)?;
    let object_repo = R2ObjectRepository::new(storage.db());
    let authority = if object_repo
        .get_mutation(record.account_id, record.resource_id, &record.object_key)?
        .is_some()
    {
        object_repo.finish_put(
            record.account_id,
            record.resource_id,
            &record.object_key,
            &record.object_version,
            i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
        )?
    } else {
        object_repo
            .get(record.account_id, record.resource_id, &record.object_key)?
            .ok_or_else(metadata_invalid)?
    };
    objects::validate_object_record(&authority, metadata)?;
    let completed_metadata = serde_json::to_string(metadata).map_err(|_| protocol_error())?;
    repo.finish_complete(
        record.account_id,
        record.resource_id,
        &record.upload_id,
        &record.object_key,
        &completed_metadata,
        i64::try_from(unix_ms()?).map_err(|_| protocol_error())?,
    )?;
    Ok(())
}
