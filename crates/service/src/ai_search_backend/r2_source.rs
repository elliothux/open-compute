//! R2 source inventory, filtering, metadata observation, and reconcile dispatch.

use super::*;
use futures::{TryStreamExt as _, stream};
use open_compute_artifacts::UserObjectKey;
use open_compute_document_parser::{FilenameMatcher, ai_search_formats, canonical_content_type};
use open_compute_storage::{AiSearchR2Candidate, AiSearchSourceReference, R2ObjectRecord};

const MAX_PREFIX_OBJECTS: u32 = 100_000;
const MAX_MATCHING_OBJECTS: usize = 10_000;

enum CandidateObservation {
    Candidate(AiSearchR2Candidate),
    Unsupported,
    InvalidSize,
}

impl AiSearchBindingService {
    pub(super) async fn run_r2_reconciler(
        &self,
        record: &AiSearchInstanceRecord,
        store: &AiSearchStore,
        claim_scheduled: bool,
    ) -> Result<(), PlatformError> {
        let Some(source) = &record.r2_source else {
            return Ok(());
        };
        let inspection = store.inspect()?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let interval = config.sync_interval.ok_or_else(corrupt)?;
        let source_config_sha256 = source_observation_contract(&config)?;
        let observation_current = store.r2_source_config_observed(source_config_sha256)?;
        let Some(claim) = store.claim_due_r2_reconcile(unix_ms(), JOB_LEASE_MS, claim_scheduled)?
        else {
            return Ok(());
        };
        if !store.r2_reconcile_has_children(&claim.job_id)? {
            let result = self
                .scan_r2_candidates(
                    record,
                    &config,
                    source.bucket_resource_id,
                    store,
                    observation_current,
                )
                .await
                .and_then(|(candidates, log_messages)| {
                    store
                        .apply_r2_reconcile(
                            &claim,
                            &candidates,
                            &log_messages,
                            !observation_current,
                            source_config_sha256,
                            unix_ms(),
                        )?
                        .then_some(())
                        .ok_or_else(unavailable)
                });
            if let Err(error) = result {
                let _ = store.settle_r2_reconcile(&claim, interval, true, unix_ms());
                return Err(error);
            }
        }
        if let Err(error) = self.run_coordinator(record, store).await {
            let _ = store.settle_r2_reconcile(&claim, interval, true, unix_ms());
            return Err(error);
        }
        if !store.settle_r2_reconcile(&claim, interval, false, unix_ms())? {
            return Err(unavailable());
        }
        let latest = store.inspect()?;
        let latest_config: ResolvedAiSearchConfig =
            serde_json::from_slice(&latest.public_config_json).map_err(|_| corrupt())?;
        let latest_digest = source_observation_contract(&latest_config)?;
        if !store.r2_source_config_observed(latest_digest)? {
            store.enqueue_config_r2_reconcile(&Uuid::now_v7().to_string(), unix_ms())?;
        }
        Ok(())
    }

    async fn scan_r2_candidates(
        &self,
        record: &AiSearchInstanceRecord,
        config: &ResolvedAiSearchConfig,
        bucket_id: ResourceId,
        store: &AiSearchStore,
        observation_current: bool,
    ) -> Result<(Vec<AiSearchR2Candidate>, Vec<String>), PlatformError> {
        let params = config.source_params.as_ref().ok_or_else(corrupt)?;
        let repository = R2ObjectRepository::new(self.storage.db());
        let snapshot = repository.snapshot_prefix(
            record.resource.account_id,
            bucket_id,
            &params.prefix,
            MAX_PREFIX_OBJECTS,
        )?;
        if snapshot.len() > usize::try_from(MAX_PREFIX_OBJECTS).map_err(|_| limit())? {
            return Err(limit());
        }
        let mut selected = Vec::new();
        let mut excluded = 0_u64;
        let mut not_included = 0_u64;
        let mut unsupported = 0_u64;
        for object in snapshot {
            if params
                .exclude_items
                .iter()
                .any(|pattern| wildcard_match(pattern.as_bytes(), object.object_key.as_bytes()))
            {
                excluded = excluded.saturating_add(1);
            } else if !params.include_items.is_empty()
                && !params
                    .include_items
                    .iter()
                    .any(|pattern| wildcard_match(pattern.as_bytes(), object.object_key.as_bytes()))
            {
                not_included = not_included.saturating_add(1);
            } else {
                selected.push(object);
            }
        }
        if selected.len() > MAX_MATCHING_OBJECTS {
            return Err(limit());
        }
        let mut existing = BTreeMap::new();
        let mut offset = 0_u64;
        loop {
            let (items, total) = store.list_items(offset, 100)?;
            for item in items {
                if item.source_kind == "r2" {
                    existing.insert(item.key.clone(), item);
                }
            }
            offset = offset.saturating_add(100);
            if offset >= total {
                break;
            }
        }
        let concurrency = usize::try_from(
            self.r2_config
                .as_ref()
                .ok_or_else(unavailable)?
                .max_metadata_head_concurrency,
        )
        .map_err(|_| limit())?;
        let existing = &existing;
        let observed: Vec<CandidateObservation> = stream::iter(selected)
            .map(|object| async move {
                if observation_current
                    && let Some(item) = existing.get(&object.object_key)
                    && let AiSearchSourceReference::R2(source) = &item.source
                    && source.object_version == object.object_version
                    && item.status == "completed"
                {
                    return Ok(CandidateObservation::Candidate(AiSearchR2Candidate {
                        item_id: item.id.clone(),
                        key: item.key.clone(),
                        object_version: source.object_version.clone(),
                        etag: source.etag.clone(),
                        object_size: source.object_size,
                        content_type: item.content_type.clone(),
                        uploaded_at_ms: source.uploaded_at_ms,
                        metadata_json: item.metadata_json.clone(),
                    }));
                }
                self.observe_r2_candidate(record, config, bucket_id, &object)
                    .await
            })
            .buffer_unordered(concurrency)
            .try_collect()
            .await?;
        let skipped_size = observed
            .iter()
            .filter(|candidate| matches!(candidate, CandidateObservation::InvalidSize))
            .count();
        unsupported = unsupported.saturating_add(
            u64::try_from(
                observed
                    .iter()
                    .filter(|candidate| matches!(candidate, CandidateObservation::Unsupported))
                    .count(),
            )
            .map_err(|_| limit())?,
        );
        let mut candidates = observed
            .into_iter()
            .filter_map(|candidate| match candidate {
                CandidateObservation::Candidate(candidate) => Some(candidate),
                CandidateObservation::Unsupported | CandidateObservation::InvalidSize => None,
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.key.cmp(&right.key));
        let mut logs = Vec::new();
        push_skip_log(&mut logs, "r2_skipped_by_exclude", excluded);
        push_skip_log(&mut logs, "r2_skipped_by_include", not_included);
        push_skip_log(&mut logs, "r2_skipped_unsupported_format", unsupported);
        push_skip_log(
            &mut logs,
            "r2_skipped_empty_or_oversize",
            u64::try_from(skipped_size).map_err(|_| limit())?,
        );
        Ok((candidates, logs))
    }

    async fn observe_r2_candidate(
        &self,
        instance: &AiSearchInstanceRecord,
        config: &ResolvedAiSearchConfig,
        bucket_id: ResourceId,
        snapshot: &R2ObjectRecord,
    ) -> Result<CandidateObservation, PlatformError> {
        let r2 = self.r2_objects.as_ref().ok_or_else(unavailable)?;
        let repository = R2ObjectRepository::new(self.storage.db());
        if repository
            .get_mutation(
                instance.resource.account_id,
                bucket_id,
                &snapshot.object_key,
            )?
            .is_some()
        {
            return Err(unavailable());
        }
        let current = repository
            .get(
                instance.resource.account_id,
                bucket_id,
                &snapshot.object_key,
            )?
            .ok_or_else(unavailable)?;
        if current.object_version != snapshot.object_version {
            return Err(unavailable());
        }
        let bucket = R2BucketRepository::new(self.storage.db())
            .get(instance.resource.account_id, bucket_id)?;
        let locator = r2.locator(bucket_id, &bucket.physical_prefix)?;
        let key = UserObjectKey::parse(&snapshot.object_key)?;
        let ssec = crate::r2_backend::objects::open_object_ssec(&self.storage, &current)?;
        let metadata = tokio::time::timeout(
            Duration::from_millis(
                self.r2_config
                    .as_ref()
                    .ok_or_else(unavailable)?
                    .operation_timeout_ms,
            ),
            r2.head(&locator, &key, ssec.as_ref()),
        )
        .await
        .map_err(|_| unavailable())??
        .ok_or_else(unavailable)?;
        crate::r2_backend::objects::validate_object_record(&current, &metadata)?;
        if metadata.version != snapshot.object_version {
            return Err(unavailable());
        }
        if metadata.size == 0 || metadata.size > self.parser.max_input_bytes() {
            return Ok(CandidateObservation::InvalidSize);
        }
        let declared_content_type = metadata
            .http_metadata
            .as_ref()
            .and_then(|http| http.content_type.as_deref());
        let Some(content_type) =
            supported_r2_content_type(&snapshot.object_key, declared_content_type)
        else {
            return Ok(CandidateObservation::Unsupported);
        };
        let custom = metadata
            .custom_metadata
            .as_ref()
            .cloned()
            .unwrap_or_default();
        let metadata_json = materialize_r2_metadata(config, &custom)?;
        Ok(CandidateObservation::Candidate(AiSearchR2Candidate {
            item_id: r2_item_id(instance.resource.id, bucket_id, &snapshot.object_key),
            key: snapshot.object_key.clone(),
            object_version: snapshot.object_version.clone(),
            etag: metadata.etag,
            object_size: metadata.size,
            content_type,
            uploaded_at_ms: metadata.uploaded,
            metadata_json,
        }))
    }

    pub(super) async fn observe_r2_item_candidate(
        &self,
        instance: &AiSearchInstanceRecord,
        config: &ResolvedAiSearchConfig,
        bucket_id: ResourceId,
        key: &str,
    ) -> Result<Option<AiSearchR2Candidate>, PlatformError> {
        let repository = R2ObjectRepository::new(self.storage.db());
        if repository
            .get_mutation(instance.resource.account_id, bucket_id, key)?
            .is_some()
        {
            return Err(unavailable());
        }
        let Some(snapshot) = repository.get(instance.resource.account_id, bucket_id, key)? else {
            return Ok(None);
        };
        let params = config.source_params.as_ref().ok_or_else(corrupt)?;
        if !source_path_matches(key, params) {
            return Ok(None);
        }
        Ok(
            match self
                .observe_r2_candidate(instance, config, bucket_id, &snapshot)
                .await?
            {
                CandidateObservation::Candidate(candidate) => Some(candidate),
                CandidateObservation::Unsupported | CandidateObservation::InvalidSize => None,
            },
        )
    }
}

fn source_observation_contract(config: &ResolvedAiSearchConfig) -> Result<[u8; 32], PlatformError> {
    let bytes = serde_json::to_vec(&(&config.source_params, &config.custom_metadata))
        .map_err(|_| corrupt())?;
    Ok(Sha256::digest(bytes).into())
}

fn push_skip_log(logs: &mut Vec<String>, reason: &str, count: u64) {
    if count != 0 {
        logs.push(format!("{reason}:{count}"));
    }
}

fn source_path_matches(key: &str, params: &crate::ai_search_config::AiSearchSourceParams) -> bool {
    if !key.starts_with(&params.prefix) {
        return false;
    }
    if params
        .exclude_items
        .iter()
        .any(|pattern| wildcard_match(pattern.as_bytes(), key.as_bytes()))
    {
        return false;
    }
    params.include_items.is_empty()
        || params
            .include_items
            .iter()
            .any(|pattern| wildcard_match(pattern.as_bytes(), key.as_bytes()))
}

fn wildcard_match(pattern: &[u8], path: &[u8]) -> bool {
    let mut states = BTreeSet::from([0_usize]);
    for &byte in path {
        let mut next = BTreeSet::new();
        for &state in &states {
            if state >= pattern.len() {
                continue;
            }
            if pattern[state] == b'*' {
                let double = pattern.get(state + 1) == Some(&b'*');
                if double || byte != b'/' {
                    next.insert(state);
                }
                let after = state + usize::from(double) + 1;
                if after < pattern.len() && pattern[after] == byte {
                    next.insert(after + 1);
                }
            } else if pattern[state] == byte {
                next.insert(state + 1);
            }
        }
        states = epsilon_closure(pattern, next);
        if states.is_empty() {
            return false;
        }
    }
    epsilon_closure(pattern, states)
        .iter()
        .any(|state| *state == pattern.len())
}

fn epsilon_closure(pattern: &[u8], mut states: BTreeSet<usize>) -> BTreeSet<usize> {
    loop {
        let mut changed = false;
        for state in states.clone() {
            if pattern.get(state) == Some(&b'*') {
                let next = state + usize::from(pattern.get(state + 1) == Some(&b'*')) + 1;
                changed |= states.insert(next);
            }
        }
        if !changed {
            return states;
        }
    }
}

fn supported_content_type(key: &str) -> Option<&'static str> {
    let basename = key.rsplit('/').next().unwrap_or(key);
    ai_search_formats().into_iter().find_map(|format| {
        let matched = match format.matcher {
            FilenameMatcher::Suffix(suffix) => basename
                .to_ascii_lowercase()
                .ends_with(&format!(".{suffix}")),
            FilenameMatcher::ExactBasename(name) => basename.eq_ignore_ascii_case(name),
        };
        matched.then_some(format.canonical_mime)
    })
}

fn supported_r2_content_type(key: &str, declared: Option<&str>) -> Option<String> {
    let declared = declared?;
    let declared = canonical_content_type(declared).ok()?;
    if declared == "application/octet-stream" {
        return None;
    }
    let filename_type = supported_content_type(key);
    let supported = ai_search_formats().into_iter().any(|format| {
        format.mime_types.contains(&declared.as_str())
            && filename_type.is_none_or(|expected| {
                format.canonical_mime == expected || format.mime_types.contains(&expected)
            })
    });
    supported.then_some(declared)
}

fn r2_item_id(instance: ResourceId, bucket: ResourceId, key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/ai-search-r2-item/v1\0");
    digest.update(instance.to_string());
    digest.update([0]);
    digest.update(bucket.to_string());
    digest.update([0]);
    digest.update(key.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn materialize_r2_metadata(
    config: &ResolvedAiSearchConfig,
    input: &BTreeMap<String, String>,
) -> Result<Vec<u8>, PlatformError> {
    let lower = input
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect::<BTreeMap<_, _>>();
    let mut output = Map::new();
    for field in &config.custom_metadata {
        let Some(text) = lower.get(&field.field_name.to_ascii_lowercase()) else {
            continue;
        };
        let value = match field.data_type {
            crate::ai_search_config::AiSearchMetadataType::Text => {
                Some(Value::String((*text).clone()))
            }
            crate::ai_search_config::AiSearchMetadataType::Number => text
                .parse::<serde_json::Number>()
                .ok()
                .filter(|number| number.as_f64().is_some_and(f64::is_finite))
                .map(Value::Number),
            crate::ai_search_config::AiSearchMetadataType::Boolean => match text.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            crate::ai_search_config::AiSearchMetadataType::Datetime => text
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|_| Value::String((*text).clone())),
        };
        if let Some(value) = value {
            output.insert(field.field_name.clone(), value);
        }
    }
    let metadata = validate_metadata(&Value::Object(output)).map_err(|_| protocol())?;
    Ok(metadata.canonical_json().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_contract_is_full_path_case_sensitive_and_distinguishes_star() {
        assert!(wildcard_match(b"**/*.pdf", b"docs/a.pdf"));
        assert!(wildcard_match(b"docs/*.pdf", b"docs/a.pdf"));
        assert!(!wildcard_match(b"docs/*.pdf", b"docs/nested/a.pdf"));
        assert!(!wildcard_match(b"**/*.pdf", b"docs/a.PDF"));
        assert!(!wildcard_match(b"*.pdf", b"docs/a.pdf"));
    }
}
