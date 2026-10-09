use super::*;
use open_compute_core::AiBackendProtocol;

/// One normalized provider score associated with the original document index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RerankResult {
    /// Zero-based index into the submitted document array.
    pub index: usize,
    /// Finite normalized relevance score in the inclusive `[0, 1]` range.
    pub relevance_score: f32,
}

/// Client frozen to one dedicated reranking model and protocol.
#[derive(Clone)]
pub struct RerankClient {
    transport: ProviderTransport,
    endpoint: url::Url,
    protocol: AiBackendProtocol,
    remote_model: String,
    provider_revision: Option<String>,
    headers: HeaderMap,
    max_request_bytes: usize,
    max_response_bytes: usize,
    timeout: Duration,
}

impl std::fmt::Debug for RerankClient {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RerankClient")
            .field("protocol", &self.protocol)
            .field("remote_model", &self.remote_model)
            .field("provider_revision", &self.provider_revision)
            .finish_non_exhaustive()
    }
}

impl RerankClient {
    /// Resolve one dedicated reranking alias without performing network I/O.
    pub fn new(config: &AiConfig, alias: &str) -> Result<Self, AiProviderError> {
        crate::tls::install_default_provider();
        config
            .validate()
            .map_err(|_| AiProviderError::ContractMismatch)?;
        let model = config
            .reranking_models
            .get(alias)
            .ok_or(AiProviderError::ContractMismatch)?;
        let backend = config
            .backends
            .get(&model.backend)
            .filter(|backend| {
                matches!(
                    backend.protocol,
                    AiBackendProtocol::CohereRerankV2 | AiBackendProtocol::RerankV1
                )
            })
            .ok_or(AiProviderError::ContractMismatch)?;
        Ok(Self {
            transport: ProviderTransport::from_process_env()
                .map_err(|_| AiProviderError::ContractMismatch)?,
            endpoint: backend
                .endpoint
                .parse()
                .map_err(|_| AiProviderError::ContractMismatch)?,
            protocol: backend.protocol,
            remote_model: model.remote_model.clone(),
            provider_revision: model.provider_revision.clone(),
            headers: resolve_backend_headers(backend, None)?,
            max_request_bytes: usize::try_from(config.max_provider_request_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            max_response_bytes: usize::try_from(config.max_provider_response_bytes)
                .map_err(|_| AiProviderError::ContractMismatch)?,
            timeout: Duration::from_millis(config.provider_timeout_ms),
        })
    }

    /// Rerank one complete bounded candidate set and return deterministic score order.
    pub async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Vec<RerankResult>, AiProviderError> {
        if query.is_empty()
            || documents.is_empty()
            || documents.len() > 50
            || documents.iter().any(String::is_empty)
        {
            return Err(AiProviderError::InvalidRequest);
        }
        let top_n = (self.protocol == AiBackendProtocol::CohereRerankV2).then_some(documents.len());
        let body = serde_json::to_vec(&RerankRequest {
            model: &self.remote_model,
            query,
            documents,
            top_n,
        })
        .map_err(|_| AiProviderError::InvalidRequest)?;
        if body.len() > self.max_request_bytes {
            return Err(AiProviderError::InvalidRequest);
        }
        let response = self
            .transport
            .request(reqwest::Method::POST, self.endpoint.clone())
            .map_err(|_| AiProviderError::ContractMismatch)?
            .header(CONTENT_TYPE, "application/json")
            .headers(self.headers.clone())
            .body(body);
        let response = tokio::time::timeout(self.timeout, response.send())
            .await
            .map_err(|_| AiProviderError::Timeout)?
            .map_err(|_| AiProviderError::Transient)?;
        let response = classify_status(response)?;
        if !content_type_is(&response, "application/json") {
            return Err(AiProviderError::MalformedResponse);
        }
        let bytes = collect_response(response, self.max_response_bytes, self.timeout).await?;
        let response: RerankResponse =
            serde_json::from_slice(&bytes).map_err(|_| AiProviderError::MalformedResponse)?;
        normalize(response.results, documents.len())
    }
}

#[derive(Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    top_n: Option<usize>,
}

#[derive(Deserialize)]
struct RerankResponse {
    results: Vec<RerankWireResult>,
}

#[derive(Deserialize)]
struct RerankWireResult {
    index: usize,
    relevance_score: f32,
}

fn normalize(
    results: Vec<RerankWireResult>,
    document_count: usize,
) -> Result<Vec<RerankResult>, AiProviderError> {
    if results.len() != document_count {
        return Err(AiProviderError::MalformedResponse);
    }
    let mut seen = vec![false; document_count];
    let mut normalized = Vec::with_capacity(document_count);
    for result in results {
        let slot = seen
            .get_mut(result.index)
            .ok_or(AiProviderError::MalformedResponse)?;
        if *slot
            || !result.relevance_score.is_finite()
            || !(0.0..=1.0).contains(&result.relevance_score)
        {
            return Err(AiProviderError::MalformedResponse);
        }
        *slot = true;
        normalized.push(RerankResult {
            index: result.index,
            relevance_score: result.relevance_score,
        });
    }
    normalized.sort_by(|left, right| {
        right
            .relevance_score
            .total_cmp(&left.relevance_score)
            .then_with(|| left.index.cmp(&right.index))
    });
    Ok(normalized)
}
