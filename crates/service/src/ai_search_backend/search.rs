//! AI Search retrieval, chat generation, and SSE response composition.

use super::*;
use futures::StreamExt as _;

impl AiSearchBindingService {
    pub(super) async fn instance_search(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let payload: SearchPayload =
            serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        self.search_record(&instance.record, &payload, None).await
    }

    pub(super) async fn namespace_search(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        require_namespace(authority)?;
        if call.instance.is_some() {
            return Err(protocol());
        }
        let payload: SearchPayload =
            serde_json::from_value(call.payload).map_err(|_| protocol())?;
        let instance_ids = payload
            .ai_search_options
            .instance_ids
            .as_ref()
            .filter(|ids| !ids.is_empty() && ids.len() <= 10)
            .cloned()
            .ok_or_else(protocol)?;
        if instance_ids.iter().collect::<BTreeSet<_>>().len() != instance_ids.len() {
            return Err(protocol());
        }
        let return_on_failure = payload.ai_search_options.return_on_failure();
        let search_query = payload.query_text()?;
        let payload = Arc::new(payload);
        let account_id = authority.binding.account_id;
        let namespace_id = authority.binding.resource.id;
        let shared_embeddings = new_query_embedding_cache();
        let results =
            futures::stream::iter(instance_ids)
                .map(|key| {
                    let payload = payload.clone();
                    let shared_embeddings = shared_embeddings.clone();
                    async move {
                        let result = match AiSearchCatalog::new(self.storage.db())
                            .get_instance_by_key(account_id, namespace_id, &key)
                        {
                            Ok(record) => match self.pins.try_pin(record.resource.id) {
                                Ok(_pin) => {
                                    self.search_record(
                                        &record,
                                        payload.as_ref(),
                                        Some(&shared_embeddings),
                                    )
                                    .await
                                }
                                Err(error) => Err(error),
                            },
                            Err(error) => Err(error),
                        };
                        (key, result)
                    }
                })
                .buffer_unordered(4)
                .collect::<Vec<_>>()
                .await;
        let mut chunks = Vec::new();
        let mut errors = Vec::new();
        for (key, result) in results {
            match result {
                Ok(value) => {
                    let values = value
                        .get("chunks")
                        .and_then(Value::as_array)
                        .ok_or_else(corrupt)?;
                    for value in values {
                        let mut value = value.as_object().cloned().ok_or_else(corrupt)?;
                        value.insert("instance_id".to_owned(), Value::String(key.clone()));
                        chunks.push(Value::Object(value));
                    }
                }
                Err(error) if return_on_failure => errors.push(json!({
                    "instance_id": key,
                    "message": error.code().as_str(),
                })),
                Err(error) => return Err(error),
            }
        }
        errors.sort_by(|left, right| {
            left["instance_id"]
                .as_str()
                .cmp(&right["instance_id"].as_str())
        });
        chunks.sort_by(|left, right| {
            let left = left.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            let right = right.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            right.total_cmp(&left)
        });
        chunks.truncate(50);
        Ok(json!({
            "search_query": search_query,
            "chunks": chunks,
            "errors": errors,
        }))
    }

    pub(super) async fn search_record(
        &self,
        record: &AiSearchInstanceRecord,
        payload: &SearchPayload,
        shared_embeddings: Option<&SharedQueryEmbeddings>,
    ) -> Result<Value, PlatformError> {
        let generation_lock = self.generation_lock(record.resource.id)?;
        let _generation = tokio::time::timeout(
            Duration::from_millis(self.ai.query_timeout_ms),
            generation_lock.read_owned(),
        )
        .await
        .map_err(|_| query_timeout())?;
        let (search_store, inspection) = self.open_store(record)?;
        let active_index_generation = inspection.active_index_generation;
        let active_epoch = inspection.active_epoch;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let mut query = payload.query_text()?;
        let rewrite = payload
            .ai_search_options
            .query_rewrite
            .as_ref()
            .and_then(|options| options.enabled)
            .unwrap_or(config.rewrite_query);
        if rewrite {
            let alias = payload
                .ai_search_options
                .query_rewrite
                .as_ref()
                .and_then(|options| options.model.as_deref())
                .or(config.rewrite_model.as_deref())
                .or(config.ai_search_model.as_deref())
                .ok_or_else(unsupported)?;
            let _permit = self.provider_permit().await?;
            query = OpenAiChatClient::new(&self.ai, alias, AiGenerationCapability::Rewrite)
                .map_err(provider_error)?
                .rewrite_query(&query)
                .await
                .map_err(provider_error)?;
        }
        let retrieval = payload.ai_search_options.retrieval.as_ref();
        if retrieval.is_some_and(|options| options.boost_by.is_some()) {
            return Err(unsupported());
        }
        let retrieval_type = retrieval
            .and_then(|options| options.retrieval_type.as_deref())
            .unwrap_or(
                if config.index_method.vector && config.index_method.keyword {
                    "hybrid"
                } else if config.index_method.vector {
                    "vector"
                } else {
                    "keyword"
                },
            );
        if matches!(retrieval_type, "vector" | "hybrid") && !config.index_method.vector
            || matches!(retrieval_type, "keyword" | "hybrid") && !config.index_method.keyword
            || !matches!(retrieval_type, "vector" | "keyword" | "hybrid")
        {
            return Err(unsupported());
        }
        let filter = if let Some(filter) = retrieval.and_then(|options| options.filters.as_ref()) {
            let indexed = config
                .custom_metadata
                .iter()
                .map(|field| field.field_name.clone())
                .collect::<BTreeSet<_>>();
            Some(compile_filter(filter, &indexed).map_err(|_| protocol())?)
        } else {
            None
        };
        let maximum = retrieval
            .and_then(|options| options.max_num_results)
            .unwrap_or(config.max_num_results);
        let threshold = retrieval
            .and_then(|options| options.match_threshold)
            .unwrap_or(config.score_threshold);
        const MAX_BRANCH_CANDIDATES: usize = 256;
        let keyword_task = if matches!(retrieval_type, "keyword" | "hybrid") {
            let mode = match retrieval.and_then(|options| options.keyword_match_mode.as_deref()) {
                Some("and") => FtsKeywordMatchMode::And,
                Some("or") => FtsKeywordMatchMode::Or,
                Some(_) => return Err(protocol()),
                None => match config.retrieval_options.keyword_match_mode {
                    Some(AiSearchKeywordMatchMode::Or) => FtsKeywordMatchMode::Or,
                    Some(AiSearchKeywordMatchMode::And) | None => FtsKeywordMatchMode::And,
                },
            };
            let fts_query = build_fts_query(&query, mode, 64).map_err(|_| protocol())?;
            let trigram = matches!(
                config.indexing_options.keyword_tokenizer,
                Some(AiSearchKeywordTokenizer::Trigram)
            );
            let query_service = self.clone();
            let query_record = record.clone();
            let keyword_filter = filter.clone();
            Some(tokio::task::spawn_blocking(move || {
                let (query_store, _) = query_service.open_store(&query_record)?;
                let mut chunks = Vec::new();
                query_store.scan_keyword_chunks_at(
                    active_index_generation,
                    &fts_query,
                    trigram,
                    |chunk| {
                        if metadata_matches(&chunk, keyword_filter.as_ref()) {
                            chunks.push(chunk);
                        }
                        Ok(chunks.len() < MAX_BRANCH_CANDIDATES)
                    },
                )?;
                Ok::<_, PlatformError>(chunks)
            }))
        } else {
            None
        };
        let mut chunks = Vec::new();
        let mut vector = Vec::new();
        if matches!(retrieval_type, "vector" | "hybrid") && inspection.active_chunk_count != 0 {
            let contract: ResolvedEmbeddingModelContract =
                serde_json::from_slice(&inspection.model_contract_json).map_err(|_| corrupt())?;
            let key = (contract.contract_sha256.clone(), query.clone());
            let query_vector = if let Some(shared) = shared_embeddings {
                cached_query_embedding(shared, key, || async {
                    let _permit = self.provider_permit().await?;
                    OpenAiProviderClient::new(&self.ai, &contract)
                        .map_err(provider_error)?
                        .embeddings(std::slice::from_ref(&query))
                        .await
                        .map_err(provider_error)?
                        .embeddings
                        .into_iter()
                        .next()
                        .ok_or_else(corrupt)
                })
                .await?
            } else {
                let _permit = self.provider_permit().await?;
                OpenAiProviderClient::new(&self.ai, &contract)
                    .map_err(provider_error)?
                    .embeddings(std::slice::from_ref(&query))
                    .await
                    .map_err(provider_error)?
                    .embeddings
                    .into_iter()
                    .next()
                    .ok_or_else(corrupt)?
            };
            let vector_service = self.clone();
            let vector_record = record.clone();
            let vector_filter = filter.clone();
            let pure_threshold = (retrieval_type == "vector").then_some(threshold as f32);
            let ranked = tokio::task::spawn_blocking(move || {
                let (vector_store, _) = vector_service.open_store(&vector_record)?;
                let mut ranked = Vec::<(RankedCandidate, AiSearchChunkRecord)>::new();
                vector_store.scan_active_chunks_at(active_index_generation, |chunk| {
                    if !metadata_matches(&chunk, vector_filter.as_ref()) {
                        return Ok(());
                    }
                    let embedding = chunk.embedding.as_ref().ok_or_else(corrupt)?;
                    let cosine =
                        cosine_similarity(&query_vector, embedding).map_err(|_| corrupt())?;
                    let score = ((cosine + 1.0) / 2.0).clamp(0.0, 1.0);
                    if pure_threshold.is_some_and(|threshold| score < threshold) {
                        return Ok(());
                    }
                    ranked.push((
                        RankedCandidate {
                            chunk_id: chunk.id.clone(),
                            score,
                        },
                        chunk,
                    ));
                    ranked.sort_by(|left, right| {
                        right
                            .0
                            .score
                            .total_cmp(&left.0.score)
                            .then_with(|| left.0.chunk_id.cmp(&right.0.chunk_id))
                    });
                    ranked.truncate(MAX_BRANCH_CANDIDATES);
                    Ok(())
                })?;
                Ok::<_, PlatformError>(ranked)
            })
            .await
            .map_err(|_| unavailable())??;
            for (candidate, chunk) in ranked {
                vector.push(candidate);
                chunks.push(chunk);
            }
        }
        let mut keyword = if let Some(keyword_task) = keyword_task {
            let mut keyword_chunks = keyword_task.await.map_err(|_| unavailable())??;
            keyword_chunks.retain(|chunk| metadata_matches(chunk, filter.as_ref()));
            let ranked = keyword_chunks
                .iter()
                .enumerate()
                .map(|(rank, chunk)| RankedCandidate {
                    chunk_id: chunk.id.clone(),
                    score: 1.0 / (rank.saturating_add(1) as f32),
                })
                .collect();
            let existing = chunks
                .iter()
                .map(|chunk| chunk.id.clone())
                .collect::<BTreeSet<_>>();
            chunks.extend(
                keyword_chunks
                    .into_iter()
                    .filter(|chunk| !existing.contains(&chunk.id)),
            );
            ranked
        } else {
            Vec::new()
        };
        sort_ranked(&mut keyword);
        let configured_fusion = match retrieval
            .and_then(|options| options.fusion_method.as_deref())
            .or(match config.fusion_method {
                AiSearchFusionMethod::Max => Some("max"),
                AiSearchFusionMethod::Rrf => Some("rrf"),
            }) {
            Some("max") => FusionMethod::Maximum,
            _ => FusionMethod::ReciprocalRank,
        };
        let fusion = if retrieval_type == "hybrid" {
            configured_fusion
        } else {
            FusionMethod::Maximum
        };
        let mut fused = fuse_candidates(
            &vector,
            &keyword,
            fusion,
            usize::from(maximum),
            threshold as f32,
        )
        .map_err(|_| protocol())?;
        let rerank = payload
            .ai_search_options
            .reranking
            .as_ref()
            .and_then(|options| options.enabled)
            .unwrap_or(config.reranking);
        let context_expansion = retrieval
            .and_then(|options| options.context_expansion)
            .unwrap_or(0);
        if context_expansion > 3 {
            return Err(limit());
        }
        let by_id = chunks
            .iter()
            .map(|chunk| (chunk.id.as_str(), chunk))
            .collect::<HashMap<_, _>>();
        if rerank && !fused.is_empty() {
            let alias = payload
                .ai_search_options
                .reranking
                .as_ref()
                .and_then(|options| options.model.as_deref())
                .or(config.reranking_model.as_deref())
                .ok_or_else(unsupported)?;
            let texts = fused
                .iter()
                .map(|candidate| {
                    by_id
                        .get(candidate.chunk_id.as_str())
                        .map(|chunk| chunk.text.clone())
                        .ok_or_else(corrupt)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let _permit = self.provider_permit().await?;
            let order = OpenAiChatClient::new(&self.ai, alias, AiGenerationCapability::Rerank)
                .map_err(provider_error)?
                .rerank(&query, &texts)
                .await
                .map_err(provider_error)?;
            fused = order
                .into_iter()
                .map(|index| fused[index].clone())
                .collect();
            if let Some(threshold) = payload
                .ai_search_options
                .reranking
                .as_ref()
                .and_then(|options| options.match_threshold)
            {
                fused.retain(|candidate| f64::from(candidate.score) >= threshold);
            }
        }
        let expanded = if context_expansion == 0 || fused.is_empty() {
            HashMap::new()
        } else {
            let targets = fused
                .iter()
                .map(|candidate| {
                    by_id.get(candidate.chunk_id.as_str()).ok_or_else(corrupt)?;
                    Ok(candidate.chunk_id.clone())
                })
                .collect::<Result<Vec<_>, PlatformError>>()?;
            let context_service = self.clone();
            let context_record = record.clone();
            tokio::task::spawn_blocking(move || {
                let (context_store, _) = context_service.open_store(&context_record)?;
                let mut expanded = HashMap::new();
                for chunk_id in targets {
                    let text = context_store
                        .active_chunk_context_at(
                            active_index_generation,
                            &chunk_id,
                            context_expansion,
                        )?
                        .into_iter()
                        .map(|chunk| chunk.text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    expanded.insert(chunk_id, text);
                }
                Ok::<_, PlatformError>(expanded)
            })
            .await
            .map_err(|_| unavailable())??
        };
        let metadata_only = retrieval
            .and_then(|options| options.metadata_only)
            .unwrap_or(false);
        let result = fused
            .iter()
            .map(|candidate| {
                let chunk = by_id.get(candidate.chunk_id.as_str()).ok_or_else(corrupt)?;
                let metadata: Value =
                    serde_json::from_slice(&chunk.metadata_json).map_err(|_| corrupt())?;
                Ok(json!({
                    "id": chunk.id,
                    "type": retrieval_type,
                    "score": candidate.score,
                    "text": if metadata_only {
                        ""
                    } else {
                        expanded.get(&candidate.chunk_id).map_or(chunk.text.as_str(), String::as_str)
                    },
                    "item": {
                        "timestamp": chunk.item_created_at_ms,
                        "key": chunk.item_key,
                        "metadata": metadata,
                    },
                    "scoring_details": {
                        "vector_rank": candidate.vector_rank,
                        "vector_score": candidate.vector_score,
                        "keyword_rank": candidate.keyword_rank,
                        "keyword_score": candidate.keyword_score,
                    },
                }))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        if !search_store.active_fence_matches(active_index_generation, active_epoch)? {
            return Err(unavailable());
        }
        Ok(json!({"search_query": query, "chunks": result}))
    }
}

fn metadata_matches(chunk: &AiSearchChunkRecord, filter: Option<&FilterExpr>) -> bool {
    filter.is_none_or(|filter| {
        serde_json::from_slice::<Value>(&chunk.metadata_json)
            .ok()
            .and_then(|metadata| validate_metadata(&metadata).ok())
            .is_some_and(|metadata| filter.matches(&metadata))
    })
}

fn sort_ranked(candidates: &mut [RankedCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
}
