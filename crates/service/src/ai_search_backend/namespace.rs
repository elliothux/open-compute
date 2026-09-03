//! Namespace and instance control-plane operations.

use super::*;

impl AiSearchBindingService {
    pub(super) fn namespace_list(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        require_namespace(authority)?;
        if call.instance.is_some() {
            return Err(protocol());
        }
        let page: ListInstances = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        if page
            .order_by
            .as_deref()
            .is_some_and(|value| value != "created_at")
        {
            return Err(protocol());
        }
        let mut records = AiSearchCatalog::new(self.storage.db())
            .list_instances(authority.account_id, authority.resource.id)?;
        if let Some(search) = page.search {
            let search = search.to_lowercase();
            records.retain(|record| record.instance_key.to_lowercase().contains(&search));
        }
        records.sort_by(|left, right| {
            left.resource
                .created_at_ms
                .cmp(&right.resource.created_at_ms)
                .then_with(|| {
                    left.resource
                        .id
                        .to_string()
                        .cmp(&right.resource.id.to_string())
                })
        });
        match page.order_by_direction.as_deref().unwrap_or("asc") {
            "asc" => {}
            "desc" => records.reverse(),
            _ => return Err(protocol()),
        }
        let total = records.len();
        let (page_number, per_page, start, end) = page_bounds(page.page, page.per_page, total)?;
        let result = records[start..end]
            .iter()
            .map(|record| self.instance_info_value(record))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "result": result,
            "result_info": pagination(result.len(), page_number, per_page, total),
        }))
    }

    pub(super) fn namespace_create(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        require_namespace(authority)?;
        if call.instance.is_some() {
            return Err(protocol());
        }
        let input: AiSearchCreateInput =
            serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance_key = input.id.clone();
        let prepared = input.prepare(&self.ai)?;
        let driver = AiSearchInstanceResourceDriver::new(
            &self.storage,
            AiSearchInstanceSpec {
                namespace_resource_id: authority.resource.id,
                instance_key: instance_key.clone(),
                public_config_json: prepared.public_config_json,
                model_contract_json: prepared.model_contract_json,
                model_contract_sha256: prepared.model_contract_sha256,
                dimensions: prepared.dimensions,
                vector_enabled: prepared.vector_enabled,
                keyword_enabled: prepared.keyword_enabled,
            },
            self.storage.sqlite_busy_timeout_ms(),
        );
        ResourceController::new(&self.storage, self.pins.clone(), driver).create(
            &CreateResourceRequest {
                account_id: authority.account_id,
                kind: BindingKind::AiSearchInstance,
                name: format!("{}:{instance_key}", authority.resource.id),
                idempotency_key: format!(
                    "ai-search:{}:{}",
                    authority.resource.id, authority.request_id
                ),
                driver_schema_version: open_compute_storage::AI_SEARCH_SCHEMA_VERSION,
                request_id: authority.request_id,
                now_ms: unix_ms()?,
            },
        )?;
        let record = AiSearchCatalog::new(self.storage.db()).get_instance_by_key(
            authority.account_id,
            authority.resource.id,
            &instance_key,
        )?;
        self.instance_info_value(&record)
    }

    pub(super) async fn namespace_delete(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        require_namespace(authority)?;
        if call.instance.is_some() {
            return Err(protocol());
        }
        let input: DeleteInstance = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let record = AiSearchCatalog::new(self.storage.db()).get_instance_by_key(
            authority.account_id,
            authority.resource.id,
            &input.instance,
        )?;
        for reference in self.open_store(&record)?.0.object_references()? {
            if self
                .snapshot_pins
                .contains_object_key(&reference.object_key)?
            {
                return Err(PlatformError::new(
                    ErrorCode::ResourceReferenced,
                    "AI Search source object is pinned by a committed snapshot",
                ));
            }
        }
        self.pins
            .fence_and_wait(record.resource.id, Duration::from_secs(5))
            .await?;
        let repository = ResourceRepository::new(self.storage.db());
        let now_ms = unix_ms()?;
        let deletion = async {
            repository.begin_delete(authority.account_id, record.resource.id, now_ms)?;
            let deleting = repository.get(authority.account_id, record.resource.id)?;
            if deleting.state != ResourceState::Deleting {
                return Err(corrupt());
            }
            let (store, _) = self.open_store(&record)?;
            store.prepare_instance_delete_and_enqueue_gc(now_ms)?;
            self.drain_object_gc(&record, &store).await?;
            if store.pending_object_gc_count()? != 0 {
                return Err(unavailable());
            }
            drop(store);
            let driver = AiSearchInstanceResourceDriver::recovery(
                &self.storage,
                self.storage.sqlite_busy_timeout_ms(),
            );
            driver.begin_delete(&deleting)?;
            driver.finalize_delete(&deleting)?;
            repository.mark_tombstoned(
                authority.account_id,
                record.resource.id,
                authority.request_id,
                unix_ms()?,
            )?;
            Ok(Value::Null)
        }
        .await;
        if deletion.is_ok() {
            self.pins.retire_fence(record.resource.id);
        } else {
            self.pins.unfence(record.resource.id);
        }
        deletion
    }

    pub(super) fn instance_info_call(
        &self,
        authority: &Authority,
        call: &JsonCall,
    ) -> Result<Value, PlatformError> {
        require_empty_object(&call.payload)?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        self.instance_info_value(&instance.record)
    }

    fn instance_info_value(&self, record: &AiSearchInstanceRecord) -> Result<Value, PlatformError> {
        let (_, inspection) = self.open_store(record)?;
        let mut value: Map<String, Value> =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        value.insert("type".to_owned(), Value::String("file".to_owned()));
        value.insert("source".to_owned(), Value::String("builtin".to_owned()));
        value.insert("status".to_owned(), Value::String("ready".to_owned()));
        value.insert(
            "namespace".to_owned(),
            Value::String(record.namespace_resource_id.to_string()),
        );
        Ok(Value::Object(value))
    }

    pub(super) fn official_instance_info_value(
        &self,
        record: &AiSearchInstanceRecord,
    ) -> Result<Value, PlatformError> {
        let (_, inspection) = self.open_store(record)?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let namespace = ResourceRepository::new(self.storage.db())
            .get(record.resource.account_id, record.namespace_resource_id)?;
        let created_at = timestamp(record.resource.created_at_ms)?;
        let modified_at = timestamp(record.resource.updated_at_ms)?;
        let identity = json!({
            "id": config.id,
            "engine_version": 1,
            "enable": true,
            "type": Value::Null,
            "source": Value::Null,
            "source_params": Value::Null,
            "ai_gateway_id": Value::Null,
            "token_id": Value::Null,
            "cache": false,
            "cache_ttl": 172_800,
        });
        let behavior = json!({
            "rewrite_query": config.rewrite_query,
            "reranking": config.reranking,
            "index_method": config.index_method,
            "hybrid_search_enabled": config.index_method.vector && config.index_method.keyword,
            "fusion_method": config.fusion_method,
            "indexing_options": config.indexing_options,
            "retrieval_options": config.retrieval_options,
            "embedding_model": config.embedding_model,
            "rewrite_model": config.rewrite_model,
            "ai_search_model": config.ai_search_model,
            "reranking_model": config.reranking_model,
        });
        let retrieval = json!({
            "chunk_size": config.chunk_size,
            "chunk_overlap": config.chunk_overlap,
            "score_threshold": config.score_threshold,
            "max_num_results": config.max_num_results,
            "public_endpoint_id": Value::Null,
            "public_endpoint_params": Value::Null,
            "paused": false,
            "status": "ready",
        });
        let lifecycle = json!({
            "metadata": config.metadata,
            "custom_metadata": config.custom_metadata,
            "sync_interval": 21_600,
            "created_at": created_at,
            "created_by": Value::Null,
            "modified_at": modified_at,
            "modified_by": Value::Null,
            "last_activity": modified_at,
            "namespace": namespace.name,
        });
        let mut output = Map::new();
        for section in [identity, behavior, retrieval, lifecycle] {
            let Value::Object(section) = section else {
                return Err(corrupt());
            };
            output.extend(section);
        }
        Ok(Value::Object(output))
    }

    pub(super) fn instance_stats(
        &self,
        authority: &Authority,
        call: &JsonCall,
    ) -> Result<Value, PlatformError> {
        require_empty_object(&call.payload)?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, inspection) = self.open_store(&instance.record)?;
        let mut counts = BTreeMap::<String, u64>::new();
        let mut item_offset = 0_u64;
        loop {
            let (items, total) = store.list_items(item_offset, 100)?;
            for item in &items {
                *counts.entry(item.status.clone()).or_default() += 1;
            }
            item_offset = item_offset
                .checked_add(u64::try_from(items.len()).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if item_offset >= total {
                break;
            }
        }
        let mut job_counts = [0_u64; 8];
        let mut offset = 0_u64;
        loop {
            let (jobs, total) = store.list_jobs(offset, 100)?;
            for job in &jobs {
                let index = match job.state.as_str() {
                    "queued" => 0,
                    "claimed" => 1,
                    "retry_wait" => 2,
                    "completed" => 3,
                    "error" => 4,
                    "cancelling" => 5,
                    "cancelled" => 6,
                    "outdated" => 7,
                    _ => return Err(corrupt()),
                };
                job_counts[index] = job_counts[index].saturating_add(1);
            }
            offset = offset
                .checked_add(u64::try_from(jobs.len()).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if offset >= total {
                break;
            }
        }
        if let Some(metrics) = &self.metrics {
            metrics.set_ai_search_jobs(job_counts);
        }
        Ok(json!({
            "queued": counts.get("queued").copied().unwrap_or(0),
            "running": counts.get("running").copied().unwrap_or(0),
            "completed": counts.get("completed").copied().unwrap_or(0),
            "error": counts.get("error").copied().unwrap_or(0),
            "skipped": counts.get("skipped").copied().unwrap_or(0),
            "outdated": counts.get("outdated").copied().unwrap_or(0),
            "engine": {
                "activeIndexGeneration": inspection.active_index_generation,
                "configGeneration": inspection.config_generation,
                "chunks": inspection.active_chunk_count,
            },
        }))
    }

    pub(super) fn official_instance_stats(
        &self,
        authority: &Authority,
        call: &JsonCall,
    ) -> Result<Value, PlatformError> {
        require_empty_object(&call.payload)?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, inspection) = self.open_store(&instance.record)?;
        let mut counts = BTreeMap::<String, u64>::new();
        let mut payload_bytes = 0_u64;
        let mut metadata_bytes = 0_u64;
        let mut object_count = 0_u64;
        let mut item_offset = 0_u64;
        loop {
            let (items, total) = store.list_items(item_offset, 100)?;
            for item in &items {
                *counts.entry(item.status.clone()).or_default() += 1;
                payload_bytes = payload_bytes
                    .checked_add(item.object.object_size)
                    .ok_or_else(limit)?;
                metadata_bytes = metadata_bytes
                    .checked_add(u64::try_from(item.metadata_json.len()).map_err(|_| limit())?)
                    .ok_or_else(limit)?;
                object_count = object_count.checked_add(1).ok_or_else(limit)?;
            }
            item_offset = item_offset
                .checked_add(u64::try_from(items.len()).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if item_offset >= total {
                break;
            }
        }
        let mut job_counts = [0_u64; 8];
        let mut offset = 0_u64;
        loop {
            let (jobs, total) = store.list_jobs(offset, 100)?;
            for job in &jobs {
                let index = match job.state.as_str() {
                    "queued" => 0,
                    "claimed" => 1,
                    "retry_wait" => 2,
                    "completed" => 3,
                    "error" => 4,
                    "cancelling" => 5,
                    "cancelled" => 6,
                    "outdated" => 7,
                    _ => return Err(corrupt()),
                };
                job_counts[index] = job_counts[index].saturating_add(1);
            }
            offset = offset
                .checked_add(u64::try_from(jobs.len()).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if offset >= total {
                break;
            }
        }
        if let Some(metrics) = &self.metrics {
            metrics.set_ai_search_jobs(job_counts);
        }
        Ok(json!({
            "queued": counts.get("queued").copied().unwrap_or(0),
            "running": counts.get("running").copied().unwrap_or(0),
            "completed": counts.get("completed").copied().unwrap_or(0),
            "error": counts.get("error").copied().unwrap_or(0),
            "skipped": counts.get("skipped").copied().unwrap_or(0),
            "outdated": counts.get("outdated").copied().unwrap_or(0),
            "degraded": false,
            "file_embed_errors": {},
            "index_source_errors": {},
            "last_activity": timestamp(instance.record.resource.updated_at_ms)?,
            "engine": {
                "r2": {
                    "payloadSizeBytes": payload_bytes,
                    "metadataSizeBytes": metadata_bytes,
                    "objectCount": object_count,
                },
                "vectorize": {
                    "vectorsCount": inspection.active_chunk_count,
                    "dimensions": store.dimensions(),
                },
            },
        }))
    }

    pub(super) async fn instance_update(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, inspection) = self.open_store(&instance.record)?;
        let patch = call.payload.as_object().ok_or_else(protocol)?;
        const REINDEX_FIELDS: &[&str] = &[
            "embedding_model",
            "index_method",
            "indexing_options",
            "chunk",
            "chunk_size",
            "chunk_overlap",
            "custom_metadata",
        ];
        let requires_reindex = patch
            .keys()
            .any(|key| REINDEX_FIELDS.contains(&key.as_str()));
        let mut merged: Map<String, Value> =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        for (key, value) in patch {
            merged.insert(key.clone(), value.clone());
        }
        if !patch.contains_key("fusion_method") {
            let hybrid = merged
                .get("index_method")
                .and_then(Value::as_object)
                .is_some_and(|index| {
                    index.get("vector").and_then(Value::as_bool) == Some(true)
                        && index.get("keyword").and_then(Value::as_bool) == Some(true)
                });
            if !hybrid {
                merged.remove("fusion_method");
            }
        }
        let input: AiSearchCreateInput =
            serde_json::from_value(Value::Object(merged)).map_err(|_| protocol())?;
        let prepared = input.prepare(&self.ai)?;
        if requires_reindex {
            let target_index_generation = inspection
                .active_index_generation
                .checked_add(1)
                .ok_or_else(limit)?;
            let now_ms = unix_ms()?;
            if !store.begin_full_reindex(
                inspection.config_generation,
                &AiSearchInstanceStorageContract {
                    resource_id: &instance.record.resource.id.to_string(),
                    model_contract_sha256: prepared.model_contract_sha256,
                    model_contract_json: &prepared.model_contract_json,
                    public_config_json: &prepared.public_config_json,
                    dimensions: prepared.dimensions,
                    vector_enabled: prepared.vector_enabled,
                    keyword_enabled: prepared.keyword_enabled,
                },
                &Uuid::now_v7().to_string(),
                now_ms,
            )? {
                return Err(unavailable());
            }
            drop(store);
            if !AiSearchCatalog::new(self.storage.db()).update_model_contract(
                instance.record.resource.account_id,
                instance.record.resource.id,
                instance.record.model_contract_sha256,
                prepared.model_contract_sha256,
            )? {
                return Err(corrupt());
            }
            let record = AiSearchCatalog::new(self.storage.db()).get_instance(
                instance.record.resource.account_id,
                instance.record.resource.id,
            )?;
            let (store, _) = self.open_store(&record)?;
            self.run_coordinator(&record, &store).await?;
            let after = store.inspect()?;
            if after.active_index_generation != target_index_generation
                || after.pending_job_count != 0
            {
                return Err(unavailable());
            }
            return self.instance_info_value(&record);
        }
        if prepared.model_contract_sha256 != instance.record.model_contract_sha256
            || prepared.vector_enabled != store.vector_enabled()
        {
            return Err(corrupt());
        }
        if !store.update_public_config(
            inspection.config_generation,
            &prepared.public_config_json,
            unix_ms()?,
        )? {
            return Err(unavailable());
        }
        self.instance_info_value(&instance.record)
    }
}
