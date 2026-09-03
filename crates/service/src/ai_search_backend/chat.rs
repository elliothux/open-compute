//! Chat completion, SSE streaming, and context budgeting.

use super::*;

impl AiSearchBindingService {
    pub(super) async fn instance_chat(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        let payload: ChatPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        if payload.stream.unwrap_or(false) {
            return Err(protocol());
        }
        let instance = self.resolve_instance(authority, call.instance.as_deref())?;
        self.chat_record(&instance.record, &payload, false).await
    }

    pub(super) async fn namespace_chat(
        &self,
        authority: &Authority,
        call: JsonCall,
    ) -> Result<Value, PlatformError> {
        require_namespace(authority)?;
        let payload: ChatPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        if payload.stream.unwrap_or(false) {
            return Err(protocol());
        }
        let record = self.namespace_chat_record(authority, &payload)?;
        let _pin = self.pins.try_pin(record.resource.id)?;
        let search = self
            .namespace_search(
                authority,
                JsonCall {
                    operation: "namespace.search".to_owned(),
                    instance: None,
                    payload: serde_json::to_value(payload.as_search()).map_err(|_| protocol())?,
                },
            )
            .await?;
        self.chat_with_search(&record, &payload, search, true).await
    }

    async fn chat_record(
        &self,
        record: &AiSearchInstanceRecord,
        payload: &ChatPayload,
        multi: bool,
    ) -> Result<Value, PlatformError> {
        let search = self
            .search_record(record, &payload.as_search(), None)
            .await?;
        self.chat_with_search(record, payload, search, multi).await
    }

    async fn chat_with_search(
        &self,
        record: &AiSearchInstanceRecord,
        payload: &ChatPayload,
        search: Value,
        multi: bool,
    ) -> Result<Value, PlatformError> {
        let (_, inspection) = self.open_store(record)?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let alias = payload
            .model
            .as_deref()
            .or(config.ai_search_model.as_deref())
            .ok_or_else(unsupported)?;
        let max_context_tokens = generation_max_context_tokens(&self.ai, alias)?;
        let messages = provider_messages(&payload.messages, &search, max_context_tokens)?;
        let _permit = self.provider_permit().await?;
        let completion = OpenAiChatClient::new(&self.ai, alias, AiGenerationCapability::Chat)
            .map_err(provider_error)?
            .chat(&messages, 1_024)
            .await
            .map_err(provider_error)?;
        let errors = search
            .get("errors")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let mut chunks = search
            .get("chunks")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(corrupt)?;
        if multi {
            for chunk in &mut chunks {
                chunk
                    .as_object_mut()
                    .ok_or_else(corrupt)?
                    .entry("instance_id".to_owned())
                    .or_insert_with(|| Value::String(record.instance_key.clone()));
            }
        }
        let mut result = json!({
            "id": Uuid::now_v7().to_string(),
            "object": "chat.completion",
            "model": alias,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": completion.content}}],
            "chunks": chunks,
        });
        if multi {
            result
                .as_object_mut()
                .ok_or_else(corrupt)?
                .insert("errors".to_owned(), errors);
        }
        Ok(result)
    }

    pub(super) async fn execute_stream(
        &self,
        authority: Authority,
        call: JsonCall,
    ) -> Result<Response, PlatformError> {
        require_permission(&authority, false)?;
        let payload: ChatPayload = serde_json::from_value(call.payload).map_err(|_| protocol())?;
        if payload.stream != Some(true) {
            return Err(protocol());
        }
        let (record, search) = match call.operation.as_str() {
            "instance.chatCompletions" => {
                let record = self
                    .resolve_instance(&authority, call.instance.as_deref())?
                    .record;
                let search = self
                    .search_record(&record, &payload.as_search(), None)
                    .await?;
                (record, search)
            }
            "namespace.chatCompletions" => {
                require_namespace(&authority)?;
                let record = self.namespace_chat_record(&authority, &payload)?;
                let search = self
                    .namespace_search(
                        &authority,
                        JsonCall {
                            operation: "namespace.search".to_owned(),
                            instance: None,
                            payload: serde_json::to_value(payload.as_search())
                                .map_err(|_| protocol())?,
                        },
                    )
                    .await?;
                (record, search)
            }
            _ => return Err(protocol()),
        };
        let child_pin = if record.resource.id == authority.resource.id {
            None
        } else {
            Some(self.pins.try_pin(record.resource.id)?)
        };
        let (_, inspection) = self.open_store(&record)?;
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&inspection.public_config_json).map_err(|_| corrupt())?;
        let alias = payload
            .model
            .as_deref()
            .or(config.ai_search_model.as_deref())
            .ok_or_else(unsupported)?;
        let max_context_tokens = generation_max_context_tokens(&self.ai, alias)?;
        let messages = provider_messages(&payload.messages, &search, max_context_tokens)?;
        let permit = self.provider_permit().await?;
        let stream = OpenAiChatClient::new(&self.ai, alias, AiGenerationCapability::Chat)
            .map_err(provider_error)?
            .chat_stream(&messages, 1_024)
            .await
            .map_err(provider_error)?;
        let chunks = search.get("chunks").ok_or_else(corrupt)?;
        let chunks_event = chunks_sse_event(chunks)?;
        let completion_id = Uuid::now_v7().to_string();
        let created = unix_ms()?.div_euclid(1_000);
        let model = alias.to_owned();
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.ai.query_timeout_ms);
        let body = futures::stream::unfold(
            (
                stream,
                0_u8,
                chunks_event,
                completion_id,
                model,
                authority,
                child_pin,
                permit,
                deadline,
            ),
            move |(
                mut stream,
                stage,
                chunks_event,
                id,
                model,
                authority,
                child_pin,
                permit,
                deadline,
            )| async move {
                if stage == 0 {
                    return Some((
                        Ok::<Bytes, std::io::Error>(chunks_event.clone()),
                        (
                            stream,
                            1,
                            chunks_event,
                            id,
                            model,
                            authority,
                            child_pin,
                            permit,
                            deadline,
                        ),
                    ));
                }
                if stage == 3 {
                    return None;
                }
                if stage == 2 {
                    return Some((
                        Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                        (
                            stream,
                            3,
                            chunks_event,
                            id,
                            model,
                            authority,
                            child_pin,
                            permit,
                            deadline,
                        ),
                    ));
                }
                let delta = tokio::time::timeout_at(deadline, stream.next_delta()).await;
                match delta {
                    Err(_) => None,
                    Ok(Ok(Some(delta))) => {
                        let event = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model,
                            "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}],
                        });
                        Some((
                            Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {event}\n\n"))),
                            (
                                stream,
                                1,
                                chunks_event,
                                id,
                                model,
                                authority,
                                child_pin,
                                permit,
                                deadline,
                            ),
                        ))
                    }
                    Ok(Ok(None)) => {
                        let event = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model,
                            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                            "usage": null,
                        });
                        Some((
                            Ok(Bytes::from(format!("data: {event}\n\n"))),
                            (
                                stream,
                                2,
                                chunks_event,
                                id,
                                model,
                                authority,
                                child_pin,
                                permit,
                                deadline,
                            ),
                        ))
                    }
                    Ok(Err(_)) => None,
                }
            },
        );
        Ok((
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(body),
        )
            .into_response())
    }

    fn namespace_chat_record(
        &self,
        authority: &Authority,
        payload: &ChatPayload,
    ) -> Result<AiSearchInstanceRecord, PlatformError> {
        let ids = payload
            .ai_search_options
            .instance_ids
            .as_ref()
            .filter(|ids| !ids.is_empty() && ids.len() <= 10)
            .ok_or_else(protocol)?;
        if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(protocol());
        }
        let return_on_failure = payload.ai_search_options.return_on_failure();
        let mut first = None;
        let mut models = BTreeSet::new();
        for id in ids {
            let record = match AiSearchCatalog::new(self.storage.db()).get_instance_by_key(
                authority.account_id,
                authority.resource.id,
                id,
            ) {
                Ok(record) => record,
                Err(_) if return_on_failure => continue,
                Err(error) => return Err(error),
            };
            if payload.model.is_none() {
                let (_, inspection) = self.open_store(&record)?;
                let config: ResolvedAiSearchConfig =
                    serde_json::from_slice(&inspection.public_config_json)
                        .map_err(|_| corrupt())?;
                models.insert(config.ai_search_model.ok_or_else(unsupported)?);
            }
            if first.is_none() {
                first = Some(record);
            }
        }
        if payload.model.is_none() && models.len() != 1 {
            return Err(unsupported());
        }
        first.ok_or_else(not_found)
    }
}

pub(super) fn chunks_sse_event(chunks: &Value) -> Result<Bytes, PlatformError> {
    if !chunks.is_array() {
        return Err(corrupt());
    }
    Ok(Bytes::from(format!("event: chunks\ndata: {chunks}\n\n")))
}

fn provider_messages(
    input: &[WireMessage],
    search: &Value,
    max_context_tokens: u32,
) -> Result<Vec<ChatMessage>, PlatformError> {
    if input.is_empty() || input.len() > 100 {
        return Err(protocol());
    }
    const RESPONSE_TOKENS: usize = 1_024;
    let maximum = usize::try_from(max_context_tokens)
        .map_err(|_| limit())?
        .saturating_sub(RESPONSE_TOKENS);
    let input_bytes = input.iter().try_fold(0_usize, |total, message| {
        total
            .checked_add(message.content.as_deref().unwrap_or_default().len())
            .ok_or_else(limit)
    })?;
    let mut remaining = maximum.checked_sub(input_bytes).ok_or_else(limit)?;
    let context = search
        .get("chunks")
        .and_then(Value::as_array)
        .ok_or_else(corrupt)?
        .iter()
        .filter_map(|chunk| chunk.get("text").and_then(Value::as_str))
        .map(|text| {
            let end = floor_char_boundary(text, remaining.min(text.len()));
            remaining = remaining.saturating_sub(end);
            &text[..end]
        })
        .take_while(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut messages = vec![ChatMessage::system(format!(
        "Use only the following retrieved context when it is relevant:\n{context}"
    ))];
    for message in input {
        let content = message.content.clone().unwrap_or_default();
        messages.push(match message.role.as_str() {
            "system" | "developer" => ChatMessage::system(content),
            "user" | "tool" => ChatMessage::user(content),
            "assistant" => ChatMessage::assistant(content),
            _ => return Err(protocol()),
        });
    }
    Ok(messages)
}

fn generation_max_context_tokens(config: &AiConfig, alias: &str) -> Result<u32, PlatformError> {
    config
        .generation_models
        .get(alias)
        .map(|model| model.max_context_tokens)
        .ok_or_else(unsupported)
}

fn floor_char_boundary(text: &str, mut end: usize) -> usize {
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}
