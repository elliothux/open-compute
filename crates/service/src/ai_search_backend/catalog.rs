//! Item and job catalog operations.

use super::*;

impl AiSearchBindingService {
    pub(super) fn items_list(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let params: ListItems = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        if params
            .sort_by
            .as_deref()
            .is_some_and(|value| !matches!(value, "status" | "modified_at"))
            || params
                .source
                .as_deref()
                .is_some_and(|value| value != "builtin")
            || params.metadata_filter.is_some()
        {
            return Err(unsupported());
        }
        let (store, _) = self.open_store(&instance.record)?;
        let mut all = Vec::new();
        let mut offset = 0_u64;
        loop {
            let (page, total) = store.list_items(offset, 100)?;
            let count = page.len();
            all.extend(page);
            offset = offset
                .checked_add(u64::try_from(count).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if offset >= total {
                break;
            }
        }
        all.retain(|item| {
            params
                .status
                .as_ref()
                .is_none_or(|status| &item.status == status)
                && params
                    .item_id
                    .as_ref()
                    .is_none_or(|item_id| &item.id == item_id)
                && params.key.as_ref().is_none_or(|key| &item.key == key)
                && params
                    .search
                    .as_ref()
                    .is_none_or(|search| item.key.contains(search))
        });
        let total = all.len();
        let (page, per_page, start, end) = page_bounds(params.page, params.per_page, total)?;
        let result = all[start..end]
            .iter()
            .map(item_info_value)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "result": result,
            "result_info": pagination(result.len(), page, per_page, total),
        }))
    }

    pub(super) fn item_info_call(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: ItemPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let item = store.get_item(&input.item_id)?.ok_or_else(not_found)?;
        item_info_value(&item)
    }

    pub(super) async fn items_delete(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: ItemPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let generation_lock = self.generation_lock(instance.record.resource.id)?;
        let _generation_guard = generation_lock.write_owned().await;
        let (store, _) = self.open_store(&instance.record)?;
        if !store.delete_item_and_enqueue_gc(&input.item_id, unix_ms()?)? {
            return Err(not_found());
        }
        self.drain_object_gc(&instance.record, &store).await?;
        Ok(Value::Null)
    }

    pub(super) fn item_logs(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: ItemLogsPayload =
            serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let limit = input.params.limit.unwrap_or(50);
        let after = input
            .params
            .cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<u64>()
            .map_err(|_| protocol())?;
        let logs = store.item_logs(&input.item_id, after, limit)?;
        let cursor = logs.last().map(|log| log.sequence.to_string());
        let result = logs
            .iter()
            .map(|log| {
                json!({
                    "timestamp": log.created_at_ms.to_string(),
                    "action": "index",
                    "message": log.message_code,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "result": result,
            "result_info": {
                "count": result.len(),
                "per_page": limit,
                "cursor": cursor,
                "truncated": result.len() == usize::try_from(limit).unwrap_or(usize::MAX),
            }
        }))
    }

    pub(super) fn item_chunks(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: ItemChunksPayload =
            serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let limit = input.params.limit.unwrap_or(50);
        let offset = input.params.offset.unwrap_or(0);
        let (chunks, total) = store.active_chunks(Some(&input.item_id), offset, limit)?;
        let result = chunks
            .iter()
            .map(|chunk| {
                Ok(json!({
                    "id": chunk.id,
                    "text": chunk.text,
                    "start_byte": chunk.start_byte,
                    "end_byte": chunk.end_byte,
                    "item": {
                        "timestamp": chunk.item_created_at_ms,
                        "key": chunk.item_key,
                        "metadata": serde_json::from_slice::<Value>(&chunk.metadata_json)
                            .map_err(|_| corrupt())?,
                    },
                }))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        Ok(json!({
            "result": result,
            "result_info": {"count": result.len(), "total": total, "limit": limit, "offset": offset},
        }))
    }

    pub(super) fn jobs_list(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let params: Page = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let page = params.page.unwrap_or(1);
        let per_page = params.per_page.unwrap_or(50);
        let offset = page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(u64::from(per_page)))
            .ok_or_else(limit)?;
        let (jobs, total) = store.list_jobs(offset, per_page)?;
        let result = jobs.iter().map(job_info_value).collect::<Vec<_>>();
        Ok(json!({
            "result": result,
            "result_info": pagination(result.len(), page, per_page, usize::try_from(total).map_err(|_| limit())?),
        }))
    }

    pub(super) async fn jobs_create(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: CreateJob = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, inspection) = self.open_store(&instance.record)?;
        if inspection.reindex_pending {
            return Err(unavailable());
        }
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let digest: [u8; 32] = Sha256::digest(&inspection.model_contract_json).into();
        let prefix = Uuid::now_v7().to_string();
        let id = format!("{prefix}-0");
        let now_ms = unix_ms()?;
        if !store.begin_full_reindex(
            inspection.config_generation,
            &AiSearchInstanceStorageContract {
                resource_id: &instance.record.resource.id.to_string(),
                model_contract_sha256: digest,
                model_contract_json: &inspection.model_contract_json,
                public_config_json: &inspection.public_config_json,
                dimensions: u32::try_from(store.dimensions()).map_err(|_| limit())?,
                vector_enabled: config.index_method.vector,
                keyword_enabled: config.index_method.keyword,
            },
            &prefix,
            now_ms,
        )? {
            return Err(unavailable());
        }
        if inspection.item_count == 0 {
            store.complete_empty_reindex(digest, now_ms)?;
            store.create_completed_job(&id, input.description.as_deref(), now_ms)?;
        }
        let job = store.get_job(&id)?.ok_or_else(corrupt)?;
        Ok(job_info_value(&job))
    }

    pub(super) fn job_info_call(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: JobPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let job = store.get_job(&input.job_id)?.ok_or_else(not_found)?;
        Ok(job_info_value(&job))
    }

    pub(super) fn job_logs(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: JobLogsPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        let page = input.params.page.unwrap_or(1);
        let per_page = input.params.per_page.unwrap_or(50);
        let offset = page
            .checked_sub(1)
            .and_then(|value| value.checked_mul(u64::from(per_page)))
            .ok_or_else(limit)?;
        let logs = store.job_logs(&input.job_id, offset, per_page)?;
        let result = logs
            .iter()
            .map(|log| {
                json!({
                    "id": log.sequence,
                    "message": log.message_code,
                    "message_type": log.message_type,
                    "created_at": log.created_at_ms,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "result": result,
            "result_info": pagination(result.len(), page, per_page, offset as usize + result.len()),
        }))
    }

    pub(super) fn job_cancel(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: JobPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, _) = self.open_store(&instance.record)?;
        if store.get_job(&input.job_id)?.is_none() {
            return Err(not_found());
        }
        store.request_cancel(&input.job_id, unix_ms()?)?;
        let job = store.get_job(&input.job_id)?.ok_or_else(corrupt)?;
        Ok(job_info_value(&job))
    }
}
