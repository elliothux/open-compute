//! Bounded OpenAI-compatible model provider client.

use crate::auth::resolve_admin_auth;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use hyper::{Method, Request, StatusCode, Uri};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_core::{
    AiAuthConfig, AiBackendConfig, AiConfig, AiGenerationCapability, ResolvedEmbeddingModelContract,
};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::time::Duration;

type ProviderTransport = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

/// Stable, content-free provider failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiProviderError {
    /// The bounded request was empty or exceeded configured admission.
    InvalidRequest,
    /// The frozen model/provider mapping changed or is unavailable.
    ContractMismatch,
    /// Provider authentication was rejected.
    Unauthorized,
    /// The provider asked the coordinator to retry after optional whole seconds.
    RateLimited {
        /// Parsed delta-seconds when present and valid.
        retry_after_seconds: Option<u64>,
    },
    /// A server or transport failure is eligible for coordinator retry.
    Transient,
    /// A non-retryable provider HTTP response was returned.
    Permanent,
    /// The bounded request deadline elapsed.
    Timeout,
    /// A successful HTTP response violated the frozen response contract.
    MalformedResponse,
}

impl Display for AiProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "AI provider request is invalid",
            Self::ContractMismatch => "AI provider model contract does not match",
            Self::Unauthorized => "AI provider authentication failed",
            Self::RateLimited { .. } => "AI provider rate limit was reached",
            Self::Transient => "AI provider is temporarily unavailable",
            Self::Permanent => "AI provider rejected the request",
            Self::Timeout => "AI provider request timed out",
            Self::MalformedResponse => "AI provider response is malformed",
        })
    }
}

impl std::error::Error for AiProviderError {}

/// Validated embedding batch returned in request order.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingBatch {
    /// One exact-dimension vector for every input string.
    pub embeddings: Vec<Vec<f32>>,
    /// Provider-reported prompt tokens, if present.
    pub prompt_tokens: Option<u64>,
}

/// One bounded OpenAI-compatible chat message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChatMessage {
    role: ChatRole,
    content: String,
}

impl ChatMessage {
    /// Construct a system instruction.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
        }
    }

    /// Construct a user message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }

    /// Construct an assistant message.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ChatRole {
    System,
    User,
    Assistant,
}

/// One validated non-stream chat completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatCompletion {
    /// Assistant text returned by the provider.
    pub content: String,
    /// Stable provider finish reason.
    pub finish_reason: String,
}

/// Client frozen to one resolved embedding contract and provider endpoint.
#[derive(Clone)]
pub struct OpenAiProviderClient {
    transport: ProviderTransport,
    endpoint: Uri,
    remote_model: String,
    contract_sha256: String,
    dimensions: usize,
    request_dimensions: Option<u32>,
    headers: HeaderMap,
    max_inputs: usize,
    max_request_bytes: usize,
    max_response_bytes: usize,
    timeout: Duration,
}

impl std::fmt::Debug for OpenAiProviderClient {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiProviderClient")
            .field("contract_sha256", &self.contract_sha256)
            .field("dimensions", &self.dimensions)
            .finish_non_exhaustive()
    }
}

impl OpenAiProviderClient {
    /// Resolve the frozen contract against the current catalog and resolve its
    /// operator credential without performing network I/O.
    pub fn new(
        config: &AiConfig,
        contract: &ResolvedEmbeddingModelContract,
    ) -> Result<Self, AiProviderError> {
        let resolved = config
            .resolve_embedding_model(Some(&contract.embedding_alias))
            .map_err(|_| AiProviderError::ContractMismatch)?;
        if &resolved != contract {
            return Err(AiProviderError::ContractMismatch);
        }
        let backend = config
            .backends
            .get(&contract.backend_name)
            .ok_or(AiProviderError::ContractMismatch)?;
        let endpoint = backend
            .endpoint
            .parse::<Uri>()
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let headers = resolve_backend_headers(backend)?;
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            transport: Client::builder(TokioExecutor::new()).build(connector),
            endpoint,
            remote_model: contract.remote_model.clone(),
            contract_sha256: contract.contract_sha256.clone(),
            dimensions: usize::try_from(contract.dimensions)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            request_dimensions: contract.send_dimensions.then_some(contract.dimensions),
            headers,
            max_inputs: usize::from(config.max_embedding_inputs_per_batch),
            max_request_bytes: usize::try_from(config.max_embedding_request_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            max_response_bytes: usize::try_from(config.max_embedding_response_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            timeout: Duration::from_millis(config.provider_timeout_ms),
        })
    }

    /// Frozen model-contract digest required for coordinator fencing.
    #[must_use]
    pub fn contract_sha256(&self) -> &str {
        &self.contract_sha256
    }

    /// Exact response vector dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Maximum provider inputs admitted in one request.
    #[must_use]
    pub const fn max_inputs_per_batch(&self) -> usize {
        self.max_inputs
    }

    /// Request one bounded ordered string batch using `encoding_format=float`.
    pub async fn embeddings(&self, inputs: &[String]) -> Result<EmbeddingBatch, AiProviderError> {
        if inputs.is_empty()
            || inputs.len() > self.max_inputs
            || inputs.iter().any(String::is_empty)
        {
            return Err(AiProviderError::InvalidRequest);
        }
        let body = serde_json::to_vec(&EmbeddingRequest {
            model: &self.remote_model,
            input: inputs,
            encoding_format: "float",
            dimensions: self.request_dimensions,
        })
        .map_err(|_| AiProviderError::InvalidRequest)?;
        if body.len() > self.max_request_bytes {
            return Err(AiProviderError::InvalidRequest);
        }
        let builder = Request::builder()
            .method(Method::POST)
            .uri(&self.endpoint)
            .header(CONTENT_TYPE, "application/json");
        let mut request = builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| AiProviderError::InvalidRequest)?;
        apply_backend_headers(&mut request, &self.headers);
        let response = tokio::time::timeout(self.timeout, self.transport.request(request))
            .await
            .map_err(|_| AiProviderError::Timeout)?
            .map_err(|_| AiProviderError::Transient)?;
        let status = response.status();
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(AiProviderError::Unauthorized);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(AiProviderError::RateLimited {
                retry_after_seconds,
            });
        }
        if status.is_server_error() {
            return Err(AiProviderError::Transient);
        }
        if status.is_redirection() || !status.is_success() {
            return Err(AiProviderError::Permanent);
        }
        let is_json = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next() == Some("application/json"));
        if !is_json {
            return Err(AiProviderError::MalformedResponse);
        }
        let bytes = tokio::time::timeout(
            self.timeout,
            Limited::new(response.into_body(), self.max_response_bytes).collect(),
        )
        .await
        .map_err(|_| AiProviderError::Timeout)?
        .map_err(|_| AiProviderError::MalformedResponse)?
        .to_bytes();
        let response: EmbeddingResponse =
            serde_json::from_slice(&bytes).map_err(|_| AiProviderError::MalformedResponse)?;
        self.validate_response(response, inputs.len())
    }

    fn validate_response(
        &self,
        response: EmbeddingResponse,
        input_count: usize,
    ) -> Result<EmbeddingBatch, AiProviderError> {
        if response.object != "list"
            || !valid_response_model(response.model.as_deref())
            || response.data.len() != input_count
        {
            return Err(AiProviderError::MalformedResponse);
        }
        let mut embeddings = vec![None; input_count];
        for item in response.data {
            if item.object != "embedding"
                || item.embedding.len() != self.dimensions
                || item.embedding.iter().any(|value| !value.is_finite())
            {
                return Err(AiProviderError::MalformedResponse);
            }
            let slot = embeddings
                .get_mut(item.index)
                .ok_or(AiProviderError::MalformedResponse)?;
            if slot.replace(item.embedding).is_some() {
                return Err(AiProviderError::MalformedResponse);
            }
        }
        if response
            .usage
            .as_ref()
            .is_some_and(|usage| usage.total_tokens < usage.prompt_tokens)
        {
            return Err(AiProviderError::MalformedResponse);
        }
        let embeddings = embeddings
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(AiProviderError::MalformedResponse)?;
        Ok(EmbeddingBatch {
            embeddings,
            prompt_tokens: response.usage.map(|usage| usage.prompt_tokens),
        })
    }
}

/// Client frozen to one configured generation alias and declared capability.
#[derive(Clone)]
pub struct OpenAiChatClient {
    transport: ProviderTransport,
    endpoint: Uri,
    remote_model: String,
    provider_revision: Option<String>,
    headers: HeaderMap,
    max_request_bytes: usize,
    max_response_bytes: usize,
    timeout: Duration,
}

impl std::fmt::Debug for OpenAiChatClient {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiChatClient")
            .field("remote_model", &self.remote_model)
            .field("provider_revision", &self.provider_revision)
            .finish_non_exhaustive()
    }
}

impl OpenAiChatClient {
    /// Resolve one generation alias that explicitly declares the required operation.
    pub fn new(
        config: &AiConfig,
        alias: &str,
        capability: AiGenerationCapability,
    ) -> Result<Self, AiProviderError> {
        config
            .validate()
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let model = config
            .generation_models
            .get(alias)
            .filter(|model| model.capabilities.contains(&capability))
            .ok_or(AiProviderError::ContractMismatch)?;
        let backend = config
            .backends
            .get(&model.backend)
            .ok_or(AiProviderError::ContractMismatch)?;
        let endpoint = backend
            .endpoint
            .parse::<Uri>()
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let headers = resolve_backend_headers(backend)?;
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            transport: Client::builder(TokioExecutor::new()).build(connector),
            endpoint,
            remote_model: model.remote_model.clone(),
            provider_revision: model.provider_revision.clone(),
            headers,
            max_request_bytes: usize::try_from(config.max_embedding_request_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            max_response_bytes: usize::try_from(config.max_embedding_response_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            timeout: Duration::from_millis(config.provider_timeout_ms),
        })
    }

    /// Execute one bounded non-stream chat completion.
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<ChatCompletion, AiProviderError> {
        let response = self.send(messages, max_tokens, false).await?;
        if !content_type_is(&response, "application/json") {
            return Err(AiProviderError::MalformedResponse);
        }
        let bytes = tokio::time::timeout(
            self.timeout,
            Limited::new(response.into_body(), self.max_response_bytes).collect(),
        )
        .await
        .map_err(|_| AiProviderError::Timeout)?
        .map_err(|_| AiProviderError::MalformedResponse)?
        .to_bytes();
        let response: ChatResponse =
            serde_json::from_slice(&bytes).map_err(|_| AiProviderError::MalformedResponse)?;
        if !valid_response_model(response.model.as_deref()) || response.choices.len() != 1 {
            return Err(AiProviderError::MalformedResponse);
        }
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or(AiProviderError::MalformedResponse)?;
        if choice.index != 0
            || choice.message.role != "assistant"
            || choice.message.content.is_empty()
            || choice.finish_reason.is_empty()
        {
            return Err(AiProviderError::MalformedResponse);
        }
        Ok(ChatCompletion {
            content: choice.message.content,
            finish_reason: choice.finish_reason,
        })
    }

    /// Start one bounded SSE completion. The caller relays deltas and must keep
    /// polling until the validated `[DONE]` marker.
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<ChatSseStream, AiProviderError> {
        let response = self.send(messages, max_tokens, true).await?;
        if !content_type_is(&response, "text/event-stream") {
            return Err(AiProviderError::MalformedResponse);
        }
        Ok(ChatSseStream {
            body: response.into_body(),
            buffer: Vec::new(),
            consumed: 0,
            maximum: self.max_response_bytes,
            done: false,
            deadline: tokio::time::Instant::now() + self.timeout,
        })
    }

    /// Rewrite one query through the same bounded chat contract.
    pub async fn rewrite_query(&self, query: &str) -> Result<String, AiProviderError> {
        let completion = self
            .chat(
                &[
                    ChatMessage::system(
                        "Rewrite the search query. Return only the rewritten query.",
                    ),
                    ChatMessage::user(query),
                ],
                256,
            )
            .await?;
        let rewritten = completion.content.trim();
        if rewritten.is_empty() {
            return Err(AiProviderError::MalformedResponse);
        }
        Ok(rewritten.to_owned())
    }

    /// Rerank a bounded candidate list through a declared rerank-capable chat model.
    /// The response must be one JSON permutation of zero-based candidate indices.
    pub async fn rerank(
        &self,
        query: &str,
        candidates: &[String],
    ) -> Result<Vec<usize>, AiProviderError> {
        if candidates.is_empty() || candidates.len() > 100 {
            return Err(AiProviderError::InvalidRequest);
        }
        let input = serde_json::to_string(&serde_json::json!({
            "query": query,
            "candidates": candidates,
        }))
        .map_err(|_| AiProviderError::InvalidRequest)?;
        let completion = self
            .chat(
                &[
                    ChatMessage::system(
                        "Rank candidates by relevance. Return only a JSON array of every candidate index.",
                    ),
                    ChatMessage::user(input),
                ],
                512,
            )
            .await?;
        let order: Vec<usize> = serde_json::from_str(&completion.content)
            .map_err(|_| AiProviderError::MalformedResponse)?;
        let mut sorted = order.clone();
        sorted.sort_unstable();
        if sorted != (0..candidates.len()).collect::<Vec<_>>() {
            return Err(AiProviderError::MalformedResponse);
        }
        Ok(order)
    }

    async fn send(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
        stream: bool,
    ) -> Result<hyper::Response<hyper::body::Incoming>, AiProviderError> {
        if messages.is_empty()
            || max_tokens == 0
            || messages
                .iter()
                .any(|message| message.content.is_empty() || message.content.len() > 1_000_000)
        {
            return Err(AiProviderError::InvalidRequest);
        }
        let body = serde_json::to_vec(&ChatRequest {
            model: &self.remote_model,
            messages,
            max_tokens,
            stream,
        })
        .map_err(|_| AiProviderError::InvalidRequest)?;
        if body.len() > self.max_request_bytes {
            return Err(AiProviderError::InvalidRequest);
        }
        let builder = Request::builder()
            .method(Method::POST)
            .uri(&self.endpoint)
            .header(CONTENT_TYPE, "application/json");
        let mut request = builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| AiProviderError::InvalidRequest)?;
        apply_backend_headers(&mut request, &self.headers);
        let response = tokio::time::timeout(self.timeout, self.transport.request(request))
            .await
            .map_err(|_| AiProviderError::Timeout)?
            .map_err(|_| AiProviderError::Transient)?;
        classify_status(response)
    }
}

/// Bounded parser over an OpenAI-compatible SSE response body.
#[derive(Debug)]
pub struct ChatSseStream {
    body: hyper::body::Incoming,
    buffer: Vec<u8>,
    consumed: usize,
    maximum: usize,
    done: bool,
    deadline: tokio::time::Instant,
}

impl ChatSseStream {
    /// Read the next validated assistant content delta. `None` is returned only
    /// after a `[DONE]` marker; EOF before that marker is malformed.
    pub async fn next_delta(&mut self) -> Result<Option<String>, AiProviderError> {
        if self.done {
            return Ok(None);
        }
        loop {
            if let Some(end) = self.buffer.windows(2).position(|window| window == b"\n\n") {
                let event = self.buffer.drain(..end + 2).collect::<Vec<_>>();
                let event =
                    std::str::from_utf8(&event).map_err(|_| AiProviderError::MalformedResponse)?;
                let data = event
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .ok_or(AiProviderError::MalformedResponse)?;
                if data == "[DONE]" {
                    self.done = true;
                    return Ok(None);
                }
                let chunk: ChatStreamChunk =
                    serde_json::from_str(data).map_err(|_| AiProviderError::MalformedResponse)?;
                if chunk.choices.len() != 1 || chunk.choices[0].index != 0 {
                    return Err(AiProviderError::MalformedResponse);
                }
                if let Some(content) = &chunk.choices[0].delta.content {
                    return Ok(Some(content.clone()));
                }
                continue;
            }
            let frame = tokio::time::timeout_at(self.deadline, self.body.frame())
                .await
                .map_err(|_| AiProviderError::Timeout)?
                .ok_or(AiProviderError::MalformedResponse)?
                .map_err(|_| AiProviderError::Transient)?;
            if let Some(data) = frame.data_ref() {
                self.consumed = self
                    .consumed
                    .checked_add(data.len())
                    .ok_or(AiProviderError::MalformedResponse)?;
                if self.consumed > self.maximum {
                    return Err(AiProviderError::MalformedResponse);
                }
                self.buffer.extend_from_slice(data);
            }
        }
    }

    /// Whether the upstream sent its terminal marker.
    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.done
    }
}

fn classify_status(
    response: hyper::Response<hyper::body::Incoming>,
) -> Result<hyper::Response<hyper::body::Incoming>, AiProviderError> {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(AiProviderError::Unauthorized);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        return Err(AiProviderError::RateLimited {
            retry_after_seconds,
        });
    }
    if status.is_server_error() {
        return Err(AiProviderError::Transient);
    }
    if status.is_redirection() || !status.is_success() {
        return Err(AiProviderError::Permanent);
    }
    Ok(response)
}

fn content_type_is(response: &hyper::Response<hyper::body::Incoming>, expected: &str) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next() == Some(expected))
}

fn resolve_backend_headers(backend: &AiBackendConfig) -> Result<HeaderMap, AiProviderError> {
    let mut headers = HeaderMap::new();
    for (name, value) in &backend.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let value = HeaderValue::from_str(value).map_err(|_| AiProviderError::ContractMismatch)?;
        headers.insert(name, value);
    }
    match &backend.auth {
        AiAuthConfig::None => {}
        AiAuthConfig::Bearer { secret } => {
            let secret =
                resolve_admin_auth(secret).map_err(|_| AiProviderError::ContractMismatch)?;
            let mut value = HeaderValue::from_str(&format!("Bearer {}", secret.expose()))
                .map_err(|_| AiProviderError::ContractMismatch)?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        AiAuthConfig::Header { name, secret } => {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| AiProviderError::ContractMismatch)?;
            let secret =
                resolve_admin_auth(secret).map_err(|_| AiProviderError::ContractMismatch)?;
            let mut value = HeaderValue::from_str(secret.expose())
                .map_err(|_| AiProviderError::ContractMismatch)?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
    }
    Ok(headers)
}

fn apply_backend_headers(request: &mut Request<Full<Bytes>>, headers: &HeaderMap) {
    for (name, value) in headers {
        request.headers_mut().insert(name, value.clone());
    }
}

fn valid_response_model(model: Option<&str>) -> bool {
    model.is_none_or(|value| {
        !value.is_empty()
            && value.len() <= 256
            && value.trim() == value
            && !value.chars().any(char::is_control)
    })
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    max_tokens: u32,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    index: usize,
    message: ChatResponseMessage,
    finish_reason: String,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatStreamChunk {
    choices: Vec<ChatStreamChoice>,
}

#[derive(Deserialize)]
struct ChatStreamChoice {
    index: usize,
    delta: ChatDelta,
}

#[derive(Deserialize)]
struct ChatDelta {
    content: Option<String>,
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<u32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    object: String,
    #[serde(default)]
    model: Option<String>,
    data: Vec<EmbeddingData>,
    #[serde(default)]
    usage: Option<EmbeddingUsage>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    object: String,
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: u64,
    total_tokens: u64,
}

#[cfg(test)]
#[path = "ai_provider_tests.rs"]
mod tests;
