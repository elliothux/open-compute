//! Authorized private backend for AI Search namespace and instance bindings.

use crate::ai_provider::{ChatMessage, OpenAiChatClient, OpenAiProviderClient};
use crate::ai_search_config::{
    AiSearchCreateInput, AiSearchFusionMethod, AiSearchKeywordMatchMode, AiSearchKeywordTokenizer,
    ResolvedAiSearchConfig, parse_keyword_only_tokenizer_contract,
};
use crate::ai_search_coordinator::{
    AiSearchCoordinator, IsolatedAiSearchDocumentParser, S3AiSearchSourceReader,
};
use crate::ai_tokenizer::AiTokenizerRegistry;
use crate::document_parser_backend::DocumentParserBindingService;
use crate::metrics::AiSearchOperation;
use crate::snapshot_pins::SnapshotPins;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use open_compute_artifacts::{AiSearchObjectRef, AiSearchObjectStore};
use open_compute_core::{
    AiConfig, AiGenerationCapability, BindingId, BindingKind, ErrorCode, PlatformError, RequestId,
    ResolvedEmbeddingModelContract, ResourceAvailability, ResourceId, ResourceState, VersionId,
};
use open_compute_search::ai_search::{
    ChunkConfig, FusionMethod, KeywordMatchMode as FtsKeywordMatchMode, RankedCandidate,
    build_fts_query, cosine_similarity, fuse_candidates,
};
use open_compute_search::{FilterExpr, compile_filter, validate_metadata};
use open_compute_storage::{
    AiSearchCatalog, AiSearchChunkRecord, AiSearchInstanceInspection, AiSearchInstanceRecord,
    AiSearchInstanceStorageContract, AiSearchItemRecord, AiSearchJobRecord, AiSearchPaths,
    AiSearchStore, AuthorizedBinding, BindingRepository, NewAiSearchItemGeneration,
    PlatformStorage, ResourceRepository,
};
use open_compute_workers::{
    AiSearchInstanceResourceDriver, AiSearchInstanceSpec, CreateResourceRequest,
    ResourceController, ResourceDriver, ResourcePin, ResourcePins,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

mod catalog;
mod chat;
mod embedding_cache;
mod ingest;
mod namespace;
mod protocol;
mod search;
mod search_types;
use embedding_cache::*;
use protocol::*;
use search_types::*;

#[cfg(test)]
#[path = "ai_search_backend_tests.rs"]
mod tests;

const CALL_PATH: &str = "/internal/ai-search/v1/call";
const STREAM_PATH: &str = "/internal/ai-search/v1/stream";
const UPLOAD_PATH: &str = "/internal/ai-search/v1/upload";
const DOWNLOAD_PATH: &str = "/internal/ai-search/v1/download";
const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.ai-search.v1+frame";
const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAME_METADATA_BYTES: usize = 64 * 1024;
const JOB_LEASE_MS: u64 = 120_000;
const JOB_RETRY_MS: u64 = 1_000;

/// Fully composed AI Search private binding service.
#[derive(Clone)]
pub(crate) struct AiSearchBindingService {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    ai: AiConfig,
    tokenizers: AiTokenizerRegistry,
    objects: AiSearchObjectStore,
    snapshot_pins: Arc<SnapshotPins>,
    parser: Arc<DocumentParserBindingService>,
    metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    maintenance_cursor: Arc<AtomicUsize>,
    provider_permits: Arc<Semaphore>,
    query_permits: Arc<Semaphore>,
    generation_locks: Arc<Mutex<HashMap<ResourceId, Arc<RwLock<()>>>>>,
}

impl std::fmt::Debug for AiSearchBindingService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiSearchBindingService")
            .field("ai", &self.ai)
            .finish_non_exhaustive()
    }
}

impl AiSearchBindingService {
    /// Compose the private plane from platform authority and fixed providers.
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        ai: AiConfig,
        objects: AiSearchObjectStore,
        snapshot_pins: Arc<SnapshotPins>,
        parser: Arc<DocumentParserBindingService>,
    ) -> Result<Self, PlatformError> {
        let tokenizers = AiTokenizerRegistry::load(&ai)?;
        let provider_permits = Arc::new(Semaphore::new(usize::from(ai.max_provider_in_flight)));
        let query_permits = Arc::new(Semaphore::new(usize::from(ai.max_provider_in_flight)));
        Ok(Self {
            storage,
            pins,
            ai,
            tokenizers,
            objects,
            snapshot_pins,
            parser,
            metrics: None,
            maintenance_cursor: Arc::new(AtomicUsize::new(0)),
            provider_permits,
            query_permits,
            generation_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Attach fixed-cardinality AI Search metrics.
    #[must_use]
    pub(crate) fn with_metrics(mut self, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_metrics_opt(
        mut self,
        metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    ) -> Self {
        self.metrics = metrics;
        self
    }

    /// Reconcile a fair bounded slice of ready instances after crashes and
    /// provider backoff, including abandoned uploads and durable object GC.
    pub(crate) async fn maintenance_once(&self) -> Result<(), PlatformError> {
        const MAX_INSTANCES_PER_TICK: usize = 16;
        const STALE_INGEST_MS: i64 = 5 * 60 * 1_000;
        let catalog = AiSearchCatalog::new(self.storage.db());
        let records = catalog.list_ready_instances()?;
        let deleting = catalog.list_deleting_instances()?;
        if records.is_empty() && deleting.is_empty() {
            return Ok(());
        }
        let mut first_error = None;
        let ready_count = records.len().min(MAX_INSTANCES_PER_TICK);
        let start = if records.is_empty() {
            0
        } else {
            self.maintenance_cursor
                .fetch_add(MAX_INSTANCES_PER_TICK, Ordering::Relaxed)
                % records.len()
        };
        for offset in 0..ready_count {
            let record = &records[(start + offset) % records.len()];
            let result = async {
                let _pin = self.pins.try_pin(record.resource.id)?;
                let current = AiSearchCatalog::new(self.storage.db())
                    .get_instance(record.resource.account_id, record.resource.id)?;
                if current.resource.state != ResourceState::Ready
                    || current.resource.spec_generation != record.resource.spec_generation
                {
                    return Ok(());
                }
                let (store, _) = self.open_store(&current)?;
                let now_ms = unix_ms()?;
                store
                    .reconcile_abandoned_ingests(now_ms.saturating_sub(STALE_INGEST_MS), now_ms)?;
                self.run_coordinator(&current, &store).await?;
                self.drain_object_gc(&current, &store).await
            }
            .await;
            let repository = ResourceRepository::new(self.storage.db());
            match result {
                Ok(()) => {
                    if record.resource.availability != ResourceAvailability::Healthy {
                        let _ = repository.set_availability(
                            record.resource.account_id,
                            record.resource.id,
                            ResourceAvailability::Healthy,
                            None,
                            unix_ms()?,
                        );
                    }
                }
                Err(error) => {
                    let _ = repository.set_availability(
                        record.resource.account_id,
                        record.resource.id,
                        ResourceAvailability::Unavailable,
                        Some("AI_SEARCH_MAINTENANCE"),
                        unix_ms()?,
                    );
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        for record in deleting
            .iter()
            .take(MAX_INSTANCES_PER_TICK.saturating_sub(ready_count))
        {
            if let Err(error) = self.resume_deleting_instance(record).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Handle one generation-authenticated request from the shared listener.
    pub(crate) async fn handle(&self, request: Request) -> Response {
        let path = request.uri().path().to_owned();
        match self.handle_inner(request).await {
            Ok(response) => {
                if path != CALL_PATH {
                    self.observe_request_path(&path, true);
                }
                response
            }
            Err(error) => {
                if path == CALL_PATH {
                    if let Some(metrics) = &self.metrics {
                        metrics.observe_ai_search_request(AiSearchOperation::Instance, false);
                    }
                } else {
                    self.observe_request_path(&path, false);
                }
                error_response(&error)
            }
        }
    }

    fn observe_request_path(&self, path: &str, success: bool) {
        let operation = if path == STREAM_PATH {
            AiSearchOperation::Chat
        } else {
            AiSearchOperation::Item
        };
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_request(operation, success);
        }
    }

    async fn handle_inner(&self, request: Request) -> Result<Response, PlatformError> {
        if request.method() != Method::POST {
            return Err(protocol());
        }
        let path = request.uri().path().to_owned();
        let authority = self.authorize(request.headers())?;
        match path.as_str() {
            CALL_PATH | STREAM_PATH => {
                if !content_type_is(request.headers(), "application/json") {
                    return Err(protocol());
                }
                let bytes = to_bytes(request.into_body(), MAX_JSON_BYTES)
                    .await
                    .map_err(|_| limit())?;
                let call: JsonCall = serde_json::from_slice(&bytes).map_err(|_| protocol())?;
                let query = matches!(
                    call.operation.as_str(),
                    "namespace.search"
                        | "namespace.chatCompletions"
                        | "instance.search"
                        | "instance.chatCompletions"
                );
                let execution = async {
                    let _query_permit = if query {
                        Some(
                            self.query_permits
                                .acquire()
                                .await
                                .map_err(|_| unavailable())?,
                        )
                    } else {
                        None
                    };
                    if path == STREAM_PATH {
                        self.execute_stream(authority, call).await
                    } else {
                        self.execute_call(authority, call).await
                    }
                };
                if query {
                    tokio::time::timeout(Duration::from_millis(self.ai.query_timeout_ms), execution)
                        .await
                        .map_err(|_| query_timeout())?
                } else {
                    execution.await
                }
            }
            UPLOAD_PATH => {
                require_permission(&authority, true)?;
                if !content_type_is(request.headers(), FRAME_CONTENT_TYPE) {
                    return Err(protocol());
                }
                let _admission = self.storage.reserve_mutation(MAX_UPLOAD_BYTES as u64)?;
                let staging = self
                    .storage
                    .data_dir()
                    .version_staging_dir()
                    .join(format!("ai-search-upload-{}", Uuid::now_v7()));
                let upload = stage_upload(request.into_body(), staging).await?;
                self.upload(authority, upload).await
            }
            DOWNLOAD_PATH => {
                require_permission(&authority, false)?;
                if !content_type_is(request.headers(), "application/json") {
                    return Err(protocol());
                }
                let bytes = to_bytes(request.into_body(), MAX_JSON_BYTES)
                    .await
                    .map_err(|_| limit())?;
                let input: ItemInput = serde_json::from_slice(&bytes).map_err(|_| protocol())?;
                self.download(authority, input).await
            }
            _ => Err(protocol()),
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<Authority, PlatformError> {
        let binding_id = parse_header::<BindingId>(headers, "x-open-compute-binding-id")?;
        let version_id = parse_header::<VersionId>(headers, "x-open-compute-version-id")?;
        let resource_id = parse_header::<ResourceId>(headers, "x-open-compute-resource-id")?;
        let generation = header_text(headers, "x-open-compute-resource-generation")?
            .parse::<u64>()
            .map_err(|_| protocol())?;
        let descriptor = parse_digest(headers, "x-open-compute-descriptor-sha256")?;
        let request_id = parse_header::<RequestId>(headers, "x-open-compute-request-id")?;
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            version_id,
            &descriptor,
        )?;
        if !matches!(
            binding.binding.kind,
            BindingKind::AiSearchNamespace | BindingKind::AiSearchInstance
        ) || binding.binding.capability_version != 1
            || binding.resource.id != resource_id
            || binding.resource.spec_generation != generation
        {
            return Err(unsupported());
        }
        let pin = self.pins.try_pin(binding.resource.id)?;
        Ok(Authority {
            binding,
            request_id,
            _bound_pin: pin,
        })
    }

    fn resolve_instance(
        &self,
        authority: &Authority,
        requested: Option<&str>,
    ) -> Result<ResolvedInstance, PlatformError> {
        let catalog = AiSearchCatalog::new(self.storage.db());
        let record = match authority.binding.binding.kind {
            BindingKind::AiSearchNamespace => {
                let key = requested.ok_or_else(protocol)?;
                catalog.get_instance_by_key(
                    authority.binding.account_id,
                    authority.binding.resource.id,
                    key,
                )?
            }
            BindingKind::AiSearchInstance if requested.is_none() => {
                catalog.get_instance(authority.binding.account_id, authority.binding.resource.id)?
            }
            _ => return Err(protocol()),
        };
        if record.resource.state != ResourceState::Ready
            || record.resource.availability != ResourceAvailability::Healthy
        {
            return Err(unavailable());
        }
        let pin = if record.resource.id == authority.binding.resource.id {
            None
        } else {
            Some(self.pins.try_pin(record.resource.id)?)
        };
        Ok(ResolvedInstance { record, _pin: pin })
    }

    fn open_store(
        &self,
        record: &AiSearchInstanceRecord,
    ) -> Result<(AiSearchStore, AiSearchInstanceInspection), PlatformError> {
        let paths = AiSearchPaths::open(self.storage.data_dir().root())?;
        let path = paths.resolve_storage_key(
            &record.storage_key,
            record.resource.account_id,
            record.resource.id,
        )?;
        let authority = open_compute_storage::ai_search::inspect_ai_search_instance(
            &path,
            &record.resource.id.to_string(),
            record.model_contract_sha256,
            self.storage.sqlite_busy_timeout_ms(),
        )?;
        if authority.model_contract_sha256 != record.model_contract_sha256 {
            let catalog = AiSearchCatalog::new(self.storage.db());
            if !catalog.update_model_contract(
                record.resource.account_id,
                record.resource.id,
                record.model_contract_sha256,
                authority.model_contract_sha256,
            )? {
                let current =
                    catalog.get_instance(record.resource.account_id, record.resource.id)?;
                if current.model_contract_sha256 != authority.model_contract_sha256 {
                    return Err(corrupt());
                }
            }
        }
        let mut inspection = authority.inspection;
        let store = AiSearchStore::open(
            &path,
            &AiSearchInstanceStorageContract {
                resource_id: &authority.resource_id,
                model_contract_sha256: authority.model_contract_sha256,
                model_contract_json: &inspection.indexing_model_contract_json,
                public_config_json: &inspection.indexing_public_config_json,
                dimensions: authority.dimensions,
                vector_enabled: authority.vector_enabled,
                keyword_enabled: authority.keyword_enabled,
            },
            record.resource.created_at_ms,
        )?;
        if inspection.reindex_pending && inspection.item_count == 0 {
            if !store.complete_empty_reindex(authority.model_contract_sha256, unix_ms()?)? {
                return Err(corrupt());
            }
            inspection = store.inspect()?;
        }
        if !inspection.reindex_pending {
            store.complete_catalog_transition(authority.model_contract_sha256, unix_ms()?)?;
        }
        Ok((store, inspection))
    }

    fn generation_lock(&self, resource_id: ResourceId) -> Result<Arc<RwLock<()>>, PlatformError> {
        let mut locks = self.generation_locks.lock().map_err(|_| corrupt())?;
        Ok(locks
            .entry(resource_id)
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone())
    }

    async fn provider_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, PlatformError> {
        self.provider_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| unavailable())
    }
}

struct Authority {
    binding: AuthorizedBinding,
    request_id: RequestId,
    _bound_pin: ResourcePin,
}

struct ResolvedInstance {
    record: AiSearchInstanceRecord,
    _pin: Option<ResourcePin>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsonCall {
    operation: String,
    instance: Option<String>,
    payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemInput {
    instance: Option<String>,
    item_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UploadHeader {
    schema_version: u16,
    instance: Option<String>,
    name: String,
    content_type: String,
    options: UploadOptions,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadOptions {
    #[serde(default)]
    metadata: Map<String, Value>,
}

#[derive(Debug)]
struct StagedUpload {
    header: UploadHeader,
    path: std::path::PathBuf,
    digest: [u8; 32],
    size: u64,
}

impl AiSearchBindingService {
    async fn execute_call(
        &self,
        authority: Authority,
        call: JsonCall,
    ) -> Result<Response, PlatformError> {
        let write = matches!(
            call.operation.as_str(),
            "namespace.create"
                | "namespace.delete"
                | "instance.update"
                | "items.delete"
                | "item.sync"
                | "jobs.create"
                | "job.cancel"
        );
        require_permission(&authority, write)?;
        let metric_operation = metric_operation(&call.operation);
        let result = match call.operation.as_str() {
            "namespace.list" => self.namespace_list(&authority, call)?,
            "namespace.create" => self.namespace_create(&authority, call)?,
            "namespace.delete" => self.namespace_delete(&authority, call).await?,
            "namespace.search" => self.namespace_search(&authority, call).await?,
            "namespace.chatCompletions" => self.namespace_chat(&authority, call).await?,
            "instance.search" => self.instance_search(&authority, call).await?,
            "instance.chatCompletions" => self.instance_chat(&authority, call).await?,
            "instance.update" => self.instance_update(&authority, call).await?,
            "instance.info" => self.instance_info_call(&authority, &call)?,
            "instance.stats" => self.instance_stats(&authority, &call)?,
            "items.list" => self.items_list(&authority, call)?,
            "items.delete" => self.items_delete(&authority, call).await?,
            "item.info" => self.item_info_call(&authority, call)?,
            "item.sync" => self.item_sync(&authority, call).await?,
            "item.logs" => self.item_logs(&authority, call)?,
            "item.chunks" => self.item_chunks(&authority, call)?,
            "jobs.list" => self.jobs_list(&authority, call)?,
            "jobs.create" => self.jobs_create(&authority, call).await?,
            "job.info" => self.job_info_call(&authority, call)?,
            "job.logs" => self.job_logs(&authority, call)?,
            "job.cancel" => self.job_cancel(&authority, call)?,
            _ => return Err(protocol()),
        };
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_request(metric_operation, true);
        }
        json_response(&json!({"schemaVersion": 1, "result": result}))
    }
}
