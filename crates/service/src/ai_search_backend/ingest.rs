//! Streaming upload, indexing orchestration, download, and object GC.

use super::*;

impl AiSearchBindingService {
    pub(super) async fn resume_deleting_instance(
        &self,
        record: &AiSearchInstanceRecord,
    ) -> Result<(), PlatformError> {
        let (store, _) = self.open_store(record)?;
        let now_ms = unix_ms()?;
        store.reconcile_abandoned_ingests(now_ms, now_ms)?;
        store.prepare_instance_delete_and_enqueue_gc(now_ms)?;
        self.drain_object_gc(record, &store).await?;
        if store.pending_object_gc_count()? != 0 {
            return Err(unavailable());
        }
        drop(store);
        let repository = ResourceRepository::new(self.storage.db());
        let deleting = repository.get(record.resource.account_id, record.resource.id)?;
        if deleting.state != ResourceState::Deleting {
            return Ok(());
        }
        let driver = AiSearchInstanceResourceDriver::recovery(
            &self.storage,
            self.storage.sqlite_busy_timeout_ms(),
        );
        driver.begin_delete(&deleting)?;
        driver.finalize_delete(&deleting)?;
        repository.mark_tombstoned(
            record.resource.account_id,
            record.resource.id,
            RequestId::generate(),
            unix_ms()?,
        )?;
        self.pins.retire_fence(record.resource.id);
        Ok(())
    }

    pub(super) async fn upload(
        &self,
        authority: Authority,
        upload: StagedUpload,
    ) -> Result<Response, PlatformError> {
        let path = upload.path.clone();
        let result = self.upload_staged(authority, upload).await;
        let cleanup = tokio::fs::remove_file(path).await;
        match result {
            Err(error) => Err(error),
            Ok(response) => {
                cleanup.map_err(|_| unavailable())?;
                Ok(response)
            }
        }
    }

    async fn upload_staged(
        &self,
        authority: Authority,
        upload: StagedUpload,
    ) -> Result<Response, PlatformError> {
        let StagedUpload {
            header,
            path: staging,
            digest,
            size,
        } = upload;
        let instance = self.resolve_instance(&authority, header.instance.as_deref())?;
        validate_source(&header.name, &header.content_type, size)?;
        let (store, inspection) = self.open_store(&instance.record)?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let metadata_json = materialize_upload_metadata(&config, &header.options.metadata)?;
        let existing = store.get_item_by_key(&header.name)?;
        let item_id = existing
            .as_ref()
            .map_or_else(|| Uuid::now_v7().to_string(), |item| item.id.clone());
        let generation = existing
            .as_ref()
            .map_or(1, |item| item.desired_generation.saturating_add(1));
        let reference = AiSearchObjectRef::new(
            authority.binding.account_id,
            instance.record.resource.id,
            digest,
            size,
        )?;
        let intent_id = Uuid::now_v7().to_string();
        let now_ms = unix_ms()?;
        let proposed_key = self.objects.object_key(&reference);
        store.reserve_ingest_intent(
            &intent_id,
            &item_id,
            &proposed_key,
            digest,
            reference.size,
            now_ms,
        )?;
        let object_key = self.objects.put_file(&reference, &staging).await?;
        if object_key != proposed_key {
            return Err(corrupt());
        }
        let job_id = Uuid::now_v7().to_string();
        if !store.mark_ingest_uploaded(&intent_id, &object_key, digest, reference.size, now_ms)? {
            return Err(corrupt());
        }
        store.commit_uploaded_ingest(
            &intent_id,
            &job_id,
            &NewAiSearchItemGeneration {
                item_id: &item_id,
                key: &header.name,
                source: "builtin",
                generation,
                index_generation: inspection.active_index_generation,
                object_key: &object_key,
                object_sha256: digest,
                object_size: reference.size,
                content_type: &header.content_type,
                metadata_json: &metadata_json,
                now_ms,
            },
        )?;
        let item = store.get_item(&item_id)?.ok_or_else(corrupt)?;
        if item.status != "queued" {
            return Err(corrupt());
        }
        let result = item_info_value(&item)?;
        json_response(&json!({"schemaVersion": 1, "result": result}))
    }

    pub(super) async fn item_sync(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let input: ItemPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        let (store, inspection) = self.open_store(&instance.record)?;
        let item = store
            .get_desired_item(&input.item_id)?
            .ok_or_else(not_found)?;
        let job_id = Uuid::now_v7().to_string();
        store.enqueue_item_generation(
            &job_id,
            &NewAiSearchItemGeneration {
                item_id: &item.id,
                key: &item.key,
                source: "builtin",
                generation: item.desired_generation.saturating_add(1),
                index_generation: inspection.active_index_generation,
                object_key: &item.object.object_key,
                object_sha256: item.object.object_sha256,
                object_size: item.object.object_size,
                content_type: &item.content_type,
                metadata_json: &item.metadata_json,
                now_ms: unix_ms()?,
            },
        )?;
        self.run_coordinator(&instance.record, &store).await?;
        let item = store.get_item(&input.item_id)?.ok_or_else(corrupt)?;
        item_info_value(&item)
    }

    pub(super) async fn run_coordinator(
        &self,
        record: &AiSearchInstanceRecord,
        store: &AiSearchStore,
    ) -> Result<(), PlatformError> {
        let inspection = store.inspect()?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.indexing_public_config_json)
                .map_err(|_| corrupt())?;
        let (tokenizer, embedding) = if config.index_method.vector {
            let contract: ResolvedEmbeddingModelContract =
                serde_json::from_slice(&inspection.indexing_model_contract_json)
                    .map_err(|_| corrupt())?;
            (
                self.tokenizers.for_contract(&contract)?,
                Some(Arc::new(
                    OpenAiProviderClient::new(&self.ai, &contract).map_err(provider_error)?,
                )
                    as Arc<dyn crate::ai_search_coordinator::AiSearchEmbedder>),
            )
        } else {
            let contract =
                parse_keyword_only_tokenizer_contract(&inspection.indexing_model_contract_json)?;
            (self.tokenizers.for_tokenizer_contract(&contract)?, None)
        };
        let mut coordinator = AiSearchCoordinator::new(
            Arc::new(S3AiSearchSourceReader::new(
                self.objects.clone(),
                record.resource.account_id,
                record.resource.id,
            )),
            Arc::new(IsolatedAiSearchDocumentParser::new(
                self.parser.clone(),
                record.resource.account_id,
            )),
            tokenizer,
            embedding,
            ChunkConfig {
                max_tokens: usize::try_from(config.chunk_size).map_err(|_| limit())?,
                overlap_tokens: usize::try_from(config.chunk_size)
                    .map_err(|_| limit())?
                    .saturating_mul(usize::from(config.chunk_overlap))
                    / 100,
            },
            JOB_LEASE_MS,
            JOB_RETRY_MS,
        )?
        .with_provider_permits(self.provider_permits.clone())
        .with_activation_lock(self.generation_lock(record.resource.id)?);
        if let Some(metrics) = &self.metrics {
            coordinator = coordinator.with_metrics(metrics.clone());
        }
        coordinator.run_until_idle(store, unix_ms()?, 32).await?;
        Ok(())
    }

    pub(super) async fn download(
        &self,
        authority: Authority,
        input: ItemInput,
    ) -> Result<Response, PlatformError> {
        let ResolvedInstance {
            record,
            _pin: child_pin,
        } = self.resolve_instance(&authority, input.instance.as_deref())?;
        let (store, _) = self.open_store(&record)?;
        let item = store.get_item(&input.item_id)?.ok_or_else(not_found)?;
        let reference = AiSearchObjectRef::new(
            authority.binding.account_id,
            record.resource.id,
            item.object.object_sha256,
            item.object.object_size,
        )?;
        let download = self
            .objects
            .download(&reference, &item.object.object_key)
            .await?;
        let filename = axum::http::HeaderValue::from_str(&item.key).map_err(|_| corrupt())?;
        let content_type =
            axum::http::HeaderValue::from_str(&item.content_type).map_err(|_| corrupt())?;
        let length =
            axum::http::HeaderValue::from_str(&download.size.to_string()).map_err(|_| corrupt())?;
        let stream = futures::stream::unfold(
            (download.body, authority, child_pin),
            |(mut body, authority, child_pin)| async move {
                body.next().await.map(|part| {
                    (
                        part.map_err(|_| std::io::Error::other("AI Search download failed")),
                        (body, authority, child_pin),
                    )
                })
            },
        );
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert("x-open-compute-filename", filename);
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length);
        Ok(response)
    }

    pub(super) async fn drain_object_gc(
        &self,
        record: &AiSearchInstanceRecord,
        store: &AiSearchStore,
    ) -> Result<(), PlatformError> {
        for _ in 0..100_000 {
            let now_ms = unix_ms()?;
            let Some(claim) = store.claim_due_object_gc(now_ms, JOB_LEASE_MS)? else {
                return Ok(());
            };
            if self.snapshot_pins.contains_object_key(&claim.object_key)? {
                if !store.retry_object_gc_claim(
                    &claim,
                    now_ms.saturating_add(i64::try_from(JOB_RETRY_MS).map_err(|_| limit())?),
                    now_ms,
                )? {
                    return Err(corrupt());
                }
                return Err(PlatformError::new(
                    ErrorCode::ResourceReferenced,
                    "AI Search source object is pinned by a committed snapshot",
                ));
            }
            let reference = AiSearchObjectRef::new(
                record.resource.account_id,
                record.resource.id,
                claim.object_sha256,
                claim.object_size,
            )?;
            match self
                .objects
                .delete_exact(&reference, &claim.object_key)
                .await
            {
                Ok(()) => {
                    if !store.complete_object_gc_claim(&claim)? {
                        return Err(corrupt());
                    }
                }
                Err(error) => {
                    store.retry_object_gc_claim(
                        &claim,
                        now_ms.saturating_add(i64::try_from(JOB_RETRY_MS).map_err(|_| limit())?),
                        now_ms,
                    )?;
                    return Err(error);
                }
            }
        }
        Err(limit())
    }
}

pub(super) fn materialize_upload_metadata(
    config: &ResolvedAiSearchConfig,
    input: &Map<String, Value>,
) -> Result<Vec<u8>, PlatformError> {
    if input.len() > 5 {
        return Err(limit());
    }
    let declarations = config
        .custom_metadata
        .iter()
        .map(|field| (field.field_name.as_str(), field.data_type))
        .collect::<HashMap<_, _>>();
    let mut output = Map::new();
    for (name, value) in input {
        let data_type = declarations.get(name.as_str()).ok_or_else(protocol)?;
        let text = value.as_str().ok_or_else(protocol)?;
        let value = match data_type {
            crate::ai_search_config::AiSearchMetadataType::Text => Value::String(text.to_owned()),
            crate::ai_search_config::AiSearchMetadataType::Number => {
                let number = text.parse::<serde_json::Number>().map_err(|_| protocol())?;
                if !number.as_f64().is_some_and(f64::is_finite) {
                    return Err(protocol());
                }
                Value::Number(number)
            }
            crate::ai_search_config::AiSearchMetadataType::Boolean => match text {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(protocol()),
            },
            crate::ai_search_config::AiSearchMetadataType::Datetime => {
                text.parse::<jiff::Timestamp>().map_err(|_| protocol())?;
                Value::String(text.to_owned())
            }
        };
        output.insert(name.clone(), value);
    }
    let metadata = validate_metadata(&Value::Object(output)).map_err(|_| protocol())?;
    Ok(metadata.canonical_json().to_vec())
}
