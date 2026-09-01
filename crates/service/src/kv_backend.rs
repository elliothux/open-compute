//! Concrete P0.4 SQLite binding executor and signed list cursors.

use crate::metrics::{KvGauge, KvGaugeGuard, KvStagingGauge, MetricsRegistry};
use base64::Engine as _;
use open_compute_core::{
    Clock, ErrorCode, KvConfig, OperationClass, PlatformError, ResourceAvailability,
};
use open_compute_storage::{
    AuthorizedBinding, KV_MAX_LIST_LIMIT, KV_MIN_CACHE_TTL_SECONDS, KV_MIN_EXPIRATION_TTL_SECONDS,
    KvEngine, KvEntry, KvEntryInfo, KvListRow, KvNamespaceRepository, KvPaths, KvPutOptions,
    PlatformStorage, ResourceRepository, canonical_metadata,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
#[cfg(any(test, feature = "test-support"))]
use std::io::{Read as _, Seek as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use std::time::UNIX_EPOCH;

const CURSOR_VERSION: u8 = 1;
const CURSOR_KEY_VERSION: u8 = 1;
const CURSOR_TTL_MS: i64 = 15 * 60 * 1000;
const CURSOR_SIGNATURE_BYTES: usize = 32;

/// Authoritative command decoded from the private adapter protocol.
#[derive(Debug)]
pub enum KvCommand {
    /// Single or multi-key snapshot read.
    Get {
        /// Ordered input keys.
        keys: Vec<String>,
        /// Compatibility-only cache TTL.
        cache_ttl: Option<u64>,
    },
    /// Atomic value replacement.
    Put {
        /// UTF-8 key.
        key: String,
        /// Exact value bytes.
        value: Vec<u8>,
        /// Absolute Unix seconds.
        expiration: Option<u64>,
        /// Relative seconds.
        expiration_ttl: Option<u64>,
        /// Metadata field value when present.
        metadata: Option<Value>,
        /// Distinguishes absent metadata from explicit JSON null.
        metadata_present: bool,
    },
    /// Atomic replacement sourced from a bounded host-owned staging file.
    PutStaged {
        /// UTF-8 key.
        key: String,
        /// Exact staged value and its validated length.
        value: KvStagedValue,
        /// Absolute Unix seconds.
        expiration: Option<u64>,
        /// Relative seconds.
        expiration_ttl: Option<u64>,
        /// Metadata field value when present.
        metadata: Option<Value>,
        /// Distinguishes absent metadata from explicit JSON null.
        metadata_present: bool,
    },
    /// Idempotent key delete.
    Delete {
        /// UTF-8 key.
        key: String,
    },
    /// Keyset-paginated list.
    List {
        /// UTF-8 prefix.
        prefix: String,
        /// Validated page size.
        limit: u16,
        /// Opaque signed cursor from a prior page.
        cursor: Option<String>,
    },
}

/// Owned, exact staging file removed automatically after the mutation attempt.
#[derive(Debug)]
pub struct KvStagedValue {
    path: PathBuf,
    file: File,
    length: usize,
    _lease: Option<KvStagingLease>,
    _staging_metric: Option<KvStagingGauge>,
}

impl KvStagedValue {
    pub(crate) fn with_lease(
        path: PathBuf,
        file: File,
        length: usize,
        lease: KvStagingLease,
    ) -> Self {
        Self {
            path,
            file,
            length,
            _lease: Some(lease),
            _staging_metric: None,
        }
    }

    pub(crate) fn with_staging_metric(mut self, metric: KvStagingGauge) -> Self {
        self._staging_metric = Some(metric);
        self
    }

    /// Read the exact staged payload for test executors and staging assertions.
    #[cfg(any(test, feature = "test-support"))]
    pub fn read_all_for_test(&mut self) -> Result<Vec<u8>, PlatformError> {
        self.file.rewind().map_err(|_| staged_unavailable())?;
        let mut value = Vec::with_capacity(self.length);
        self.file
            .by_ref()
            .take(u64::try_from(self.length).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut value)
            .map_err(|_| staged_unavailable())?;
        if value.len() != self.length {
            return Err(staged_unavailable());
        }
        Ok(value)
    }
}

/// Count permits retained until a staged mutation reaches a terminal path.
#[derive(Debug)]
pub(crate) struct KvStagingLease {
    _global: tokio::sync::OwnedSemaphorePermit,
    _resource: tokio::sync::OwnedSemaphorePermit,
}

impl KvStagingLease {
    pub(crate) const fn new(
        global: tokio::sync::OwnedSemaphorePermit,
        resource: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            _global: global,
            _resource: resource,
        }
    }
}

impl Drop for KvStagedValue {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Backend result before private wire encoding.
#[derive(Clone, Debug)]
pub enum KvCommandResult {
    /// Entries align with input keys; missing entries are `None`.
    Entries(Vec<Option<KvEntry>>),
    /// Mutation committed.
    Mutation,
    /// One list page and optional continuation cursor.
    List {
        /// Ordered live keys.
        rows: Vec<KvListRow>,
        /// True when no additional live key was observed.
        complete: bool,
        /// Present only when `complete` is false.
        cursor: Option<String>,
    },
}

/// One ordered part of a single-value response stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KvStreamPart {
    /// Entry presence and metadata, always emitted before value bytes.
    Entry(Option<KvEntryInfo>),
    /// One bounded exact value chunk.
    Bytes(Vec<u8>),
}

#[derive(Debug)]
struct KvHandle {
    engine: KvEngine,
    spec_generation: u64,
    last_used_ms: AtomicI64,
    writer: Mutex<()>,
    readers: Arc<ConnectionGate>,
    streams: Arc<ConnectionGate>,
}

impl KvHandle {
    fn touch(&self, now_ms: i64) {
        self.last_used_ms.store(now_ms, Ordering::Release);
    }
}

/// Real namespace executor composed into the private binding service.
pub struct SqliteKvBindingExecutor {
    storage: Arc<PlatformStorage>,
    clock: Arc<dyn Clock>,
    last_effective_ms: AtomicI64,
    connections: Arc<ConnectionGate>,
    handles: Mutex<HashMap<open_compute_core::ResourceId, Arc<KvHandle>>>,
    max_handles: usize,
    idle_handle_ttl_ms: i64,
    readers_per_namespace: u32,
    streams: Arc<ConnectionGate>,
    streams_per_namespace: u32,
    operation_timeout: Duration,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for SqliteKvBindingExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteKvBindingExecutor")
            .finish_non_exhaustive()
    }
}

impl SqliteKvBindingExecutor {
    /// Bind central authority, master-key cursor signing, and the host clock.
    #[must_use]
    pub fn new(storage: Arc<PlatformStorage>, clock: Arc<dyn Clock>) -> Self {
        Self::with_config(storage, clock, &KvConfig::default())
    }

    /// Bind authority with an operator-selected hard connection ceiling.
    #[must_use]
    pub fn with_connection_limit(
        storage: Arc<PlatformStorage>,
        clock: Arc<dyn Clock>,
        max_connections: u32,
    ) -> Self {
        let config = KvConfig {
            max_connections,
            ..KvConfig::default()
        };
        Self::with_config(storage, clock, &config)
    }

    /// Bind authority with the complete validated operator KV policy.
    #[must_use]
    pub fn with_config(
        storage: Arc<PlatformStorage>,
        clock: Arc<dyn Clock>,
        config: &KvConfig,
    ) -> Self {
        Self {
            storage,
            clock,
            last_effective_ms: AtomicI64::new(0),
            connections: Arc::new(ConnectionGate::new(config.max_connections.max(1))),
            handles: Mutex::new(HashMap::new()),
            max_handles: usize::try_from(config.max_connections.max(1)).unwrap_or(1),
            idle_handle_ttl_ms: i64::try_from(config.idle_handle_ttl_ms).unwrap_or(i64::MAX),
            readers_per_namespace: config.max_readers_per_namespace.max(1),
            streams: Arc::new(ConnectionGate::new(config.max_active_streams.max(1))),
            streams_per_namespace: config.max_active_streams_per_namespace.max(1),
            operation_timeout: Duration::from_millis(config.operation_timeout_ms.max(1)),
            metrics: None,
        }
    }

    pub(crate) fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    fn open_handle(
        &self,
        binding: &AuthorizedBinding,
    ) -> Result<(Arc<KvHandle>, i64), PlatformError> {
        let record = KvNamespaceRepository::new(self.storage.db())
            .get(binding.account_id, binding.resource.id)?;
        if record.resource.spec_generation != binding.binding.resource_spec_generation {
            return Err(PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "KV binding generation does not match resource authority",
            ));
        }
        let now_ms = self.effective_now_ms();
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handles.retain(|_, handle| {
            Arc::strong_count(handle) > 1
                || now_ms.saturating_sub(handle.last_used_ms.load(Ordering::Acquire))
                    < self.idle_handle_ttl_ms
        });
        if let Some(handle) = handles.get(&binding.resource.id) {
            if handle.spec_generation != record.resource.spec_generation {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "KV cached handle generation does not match authority",
                ));
            }
            handle.touch(now_ms);
            return Ok((handle.clone(), now_ms));
        }
        if handles.len() >= self.max_handles {
            let idle = handles
                .iter()
                .filter(|(_, handle)| Arc::strong_count(handle) == 1)
                .min_by_key(|(_, handle)| handle.last_used_ms.load(Ordering::Acquire))
                .map(|(resource, _)| *resource);
            let Some(idle) = idle else {
                return Err(PlatformError::new(
                    ErrorCode::KvBusy,
                    "KV handle limit is temporarily saturated",
                ));
            };
            if let Some(evicted) = handles.remove(&idle) {
                let _ = evicted.engine.checkpoint(false);
            }
        }
        let paths = KvPaths::open(self.storage.data_dir().root())?;
        let path = paths.resolve_storage_key(
            &record.storage_key,
            binding.account_id,
            binding.resource.id,
        )?;
        let handle = Arc::new(KvHandle {
            engine: KvEngine::from_record(path, &record)?,
            spec_generation: record.resource.spec_generation,
            last_used_ms: AtomicI64::new(now_ms),
            writer: Mutex::new(()),
            readers: Arc::new(ConnectionGate::new(self.readers_per_namespace)),
            streams: Arc::new(ConnectionGate::new(self.streams_per_namespace)),
        });
        handles.insert(binding.resource.id, handle.clone());
        Ok((handle, now_ms))
    }

    fn isolate_failure<T>(&self, binding: &AuthorizedBinding, result: &Result<T, PlatformError>) {
        let Err(error) = result else { return };
        if !matches!(
            error.code(),
            ErrorCode::KvCorrupt | ErrorCode::KvUnavailable
        ) {
            return;
        }
        let code = if error.code() == ErrorCode::KvCorrupt {
            "KV_CORRUPT"
        } else {
            "KV_UNAVAILABLE"
        };
        let _ = ResourceRepository::new(self.storage.db()).set_availability(
            binding.account_id,
            binding.resource.id,
            ResourceAvailability::Unavailable,
            Some(code),
            self.effective_now_ms(),
        );
    }

    fn execute_inner(
        &self,
        binding: &AuthorizedBinding,
        engine: &KvEngine,
        now_ms: i64,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        match command {
            KvCommand::Get { keys, cache_ttl } => {
                if cache_ttl.is_some_and(|value| value < KV_MIN_CACHE_TTL_SECONDS) {
                    return Err(invalid_options());
                }
                if keys.len() == 1 {
                    Ok(KvCommandResult::Entries(vec![
                        engine.get(&keys[0], now_ms)?,
                    ]))
                } else {
                    Ok(KvCommandResult::Entries(engine.get_many(&keys, now_ms)?))
                }
            }
            KvCommand::Put {
                key,
                value,
                expiration,
                expiration_ttl,
                metadata,
                metadata_present,
            } => {
                ensure_storage_headroom(&self.storage, value.len())?;
                let options = prepare_put_options(
                    now_ms,
                    expiration,
                    expiration_ttl,
                    metadata.as_ref(),
                    metadata_present,
                )?;
                engine.put(&key, &value, &options, now_ms)?;
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::PutStaged {
                key,
                mut value,
                expiration,
                expiration_ttl,
                metadata,
                metadata_present,
            } => {
                ensure_storage_headroom(&self.storage, value.length)?;
                let options = prepare_put_options(
                    now_ms,
                    expiration,
                    expiration_ttl,
                    metadata.as_ref(),
                    metadata_present,
                )?;
                engine.put_reader(&key, &mut value.file, value.length, &options, now_ms)?;
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::Delete { key } => {
                engine.delete(&key)?;
                Ok(KvCommandResult::Mutation)
            }
            KvCommand::List {
                prefix,
                limit,
                cursor,
            } => {
                if limit == 0 || limit > KV_MAX_LIST_LIMIT {
                    return Err(invalid_options());
                }
                let after = cursor
                    .as_deref()
                    .map(|cursor| self.verify_cursor(binding, &prefix, cursor, now_ms))
                    .transpose()?;
                let page = engine.list(&prefix, after.as_deref(), limit, now_ms)?;
                let cursor = if page.complete {
                    None
                } else {
                    page.rows
                        .last()
                        .map(|row| self.sign_cursor(binding, &prefix, &row.key, now_ms))
                        .transpose()?
                };
                Ok(KvCommandResult::List {
                    rows: page.rows,
                    complete: page.complete,
                    cursor,
                })
            }
        }
    }

    fn effective_now_ms(&self) -> i64 {
        let wall = self
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
            });
        let mut prior = self.last_effective_ms.load(Ordering::Acquire);
        loop {
            let effective = wall.max(prior);
            match self.last_effective_ms.compare_exchange_weak(
                prior,
                effective,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return effective,
                Err(next) => prior = next,
            }
        }
    }

    fn sign_cursor(
        &self,
        binding: &AuthorizedBinding,
        prefix: &str,
        last_key: &[u8],
        now_ms: i64,
    ) -> Result<String, PlatformError> {
        let key_len = u16::try_from(last_key.len()).map_err(|_| cursor_invalid())?;
        let mut payload = Vec::with_capacity(1 + 16 + 8 + 32 + 8 + 8 + 1 + 2 + last_key.len());
        payload.push(CURSOR_VERSION);
        payload.extend_from_slice(binding.resource.id.as_uuid().as_bytes());
        payload.extend_from_slice(&binding.resource.spec_generation.to_be_bytes());
        payload.extend_from_slice(&Sha256::digest(prefix.as_bytes()));
        payload.extend_from_slice(&now_ms.to_be_bytes());
        payload.extend_from_slice(&now_ms.saturating_add(CURSOR_TTL_MS).to_be_bytes());
        payload.push(CURSOR_KEY_VERSION);
        payload.extend_from_slice(&key_len.to_be_bytes());
        payload.extend_from_slice(last_key);
        let signature = self.storage.crypto().sign_kv_cursor(&payload);
        payload.extend_from_slice(&signature);
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload))
    }

    fn verify_cursor(
        &self,
        binding: &AuthorizedBinding,
        prefix: &str,
        cursor: &str,
        now_ms: i64,
    ) -> Result<Vec<u8>, PlatformError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cursor)
            .map_err(|_| cursor_invalid())?;
        const FIXED: usize = 1 + 16 + 8 + 32 + 8 + 8 + 1 + 2;
        if bytes.len() < FIXED + CURSOR_SIGNATURE_BYTES || bytes[0] != CURSOR_VERSION {
            return Err(cursor_invalid());
        }
        let signature_offset = bytes.len() - CURSOR_SIGNATURE_BYTES;
        let (payload, signature) = bytes.split_at(signature_offset);
        if !self.storage.crypto().verify_kv_cursor(payload, signature) {
            return Err(cursor_invalid());
        }
        let resource = uuid::Uuid::from_slice(&payload[1..17]).map_err(|_| cursor_invalid())?;
        if resource != binding.resource.id.as_uuid() {
            return Err(cursor_invalid());
        }
        let generation =
            u64::from_be_bytes(payload[17..25].try_into().map_err(|_| cursor_invalid())?);
        if generation != binding.resource.spec_generation
            || payload[25..57] != Sha256::digest(prefix.as_bytes())[..]
        {
            return Err(cursor_invalid());
        }
        let issued = i64::from_be_bytes(payload[57..65].try_into().map_err(|_| cursor_invalid())?);
        let expires = i64::from_be_bytes(payload[65..73].try_into().map_err(|_| cursor_invalid())?);
        if issued < 0 || expires != issued.saturating_add(CURSOR_TTL_MS) || now_ms > expires {
            return Err(cursor_invalid());
        }
        if payload[73] != CURSOR_KEY_VERSION {
            return Err(cursor_invalid());
        }
        let key_len = usize::from(u16::from_be_bytes(
            payload[74..76].try_into().map_err(|_| cursor_invalid())?,
        ));
        if payload.len() != FIXED + key_len {
            return Err(cursor_invalid());
        }
        let key = payload[FIXED..].to_vec();
        if key.is_empty() || std::str::from_utf8(&key).is_err() {
            return Err(cursor_invalid());
        }
        Ok(key)
    }
}

fn prepare_put_options(
    now_ms: i64,
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
    metadata: Option<&Value>,
    metadata_present: bool,
) -> Result<KvPutOptions, PlatformError> {
    let expires_at_ms = if let Some(ttl) = expiration_ttl {
        if ttl < KV_MIN_EXPIRATION_TTL_SECONDS {
            return Err(invalid_options());
        }
        let delta = i64::try_from(ttl)
            .ok()
            .and_then(|value| value.checked_mul(1000))
            .ok_or_else(invalid_options)?;
        Some(now_ms.checked_add(delta).ok_or_else(invalid_options)?)
    } else if let Some(seconds) = expiration {
        let millis = i64::try_from(seconds)
            .ok()
            .and_then(|value| value.checked_mul(1000))
            .ok_or_else(invalid_options)?;
        if millis < now_ms.saturating_add(60_000) {
            return Err(invalid_options());
        }
        Some(millis)
    } else {
        None
    };
    let metadata_json = if metadata_present {
        Some(canonical_metadata(metadata.unwrap_or(&Value::Null))?)
    } else {
        None
    };
    Ok(KvPutOptions {
        expires_at_ms,
        metadata_json,
    })
}

fn staged_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvUnavailable,
        "KV value staging file is unavailable",
    )
}

pub(crate) fn ensure_storage_headroom(
    storage: &PlatformStorage,
    additional_bytes: usize,
) -> Result<(), PlatformError> {
    let stat = rustix::fs::statvfs(storage.data_dir().root()).map_err(|_| staged_unavailable())?;
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    let required = storage
        .free_space_hard_bytes()
        .checked_add(additional_bytes as u64)
        .ok_or_else(storage_full)?;
    if available < required {
        return Err(storage_full());
    }
    Ok(())
}

fn storage_full() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvStorageFull,
        "KV filesystem free-space safety floor was reached",
    )
}

struct ConnectionGate {
    limit: u32,
    active: Mutex<u32>,
    changed: Condvar,
}

impl std::fmt::Debug for ConnectionGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionGate")
            .field("limit", &self.limit)
            .finish()
    }
}

impl ConnectionGate {
    const fn new(limit: u32) -> Self {
        Self {
            limit,
            active: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>, timeout: Duration) -> Result<ConnectionPermit, PlatformError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut active, wait) = self
            .changed
            .wait_timeout_while(active, timeout, |active| *active >= self.limit)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if wait.timed_out() && *active >= self.limit {
            return Err(PlatformError::new(
                ErrorCode::KvBusy,
                "KV connection limit is temporarily saturated",
            ));
        }
        *active += 1;
        Ok(ConnectionPermit { gate: self.clone() })
    }
}

struct ConnectionPermit {
    gate: Arc<ConnectionGate>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let mut active = self
            .gate
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *active = active.saturating_sub(1);
        self.gate.changed.notify_one();
    }
}

impl crate::binding_backend::KvBindingExecutor for SqliteKvBindingExecutor {
    fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    fn stream_limits(&self) -> (u32, u32) {
        (self.streams.limit, self.streams_per_namespace)
    }

    /// Execute one already-authorized KV command.
    fn execute(
        &self,
        binding: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        let result = (|| {
            let reservation_bytes = match &command {
                KvCommand::Put { value, .. } => Some(value.len() as u64 + 64 * 1024),
                KvCommand::PutStaged { value, .. } => Some(value.length as u64 + 64 * 1024),
                _ => None,
            };
            let admission = reservation_bytes
                .map(|bytes| self.storage.reserve_mutation(bytes))
                .transpose();
            if let Some(metrics) = &self.metrics
                && reservation_bytes.is_some()
            {
                metrics.observe_admission(
                    OperationClass::Kv,
                    admission.as_ref().err().map(PlatformError::code),
                );
            }
            let _admission = admission?;
            let mutation = matches!(
                &command,
                KvCommand::Put { .. } | KvCommand::PutStaged { .. } | KvCommand::Delete { .. }
            );
            let _connection = self.connections.acquire(self.operation_timeout)?;
            let _connection_metric = self.metrics.as_ref().map(|metrics| {
                KvGaugeGuard::new(
                    metrics,
                    if mutation {
                        KvGauge::WriterConnection
                    } else {
                        KvGauge::ReaderConnection
                    },
                )
            });
            let (handle, now_ms) = self.open_handle(binding)?;
            let _writer = mutation.then(|| {
                handle
                    .writer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            });
            let _reader = (!mutation)
                .then(|| handle.readers.acquire(self.operation_timeout))
                .transpose()?;
            KvNamespaceRepository::new(self.storage.db())
                .record_open(binding.resource.id, now_ms)?;
            let result = self.execute_inner(binding, &handle.engine, now_ms, command);
            handle.touch(self.effective_now_ms());
            result
        })();
        if let (Some(metrics), Err(error)) = (&self.metrics, &result) {
            metrics.observe_product_error(OperationClass::Kv, error.code());
        }
        self.isolate_failure(binding, &result);
        result
    }

    /// Stream a single get through bounded global and per-namespace slots.
    fn stream_get(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        cache_ttl: Option<u64>,
        sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        let result = (|| {
            if cache_ttl.is_some_and(|value| value < KV_MIN_CACHE_TTL_SECONDS) {
                return Err(invalid_options());
            }
            let _connection = self.connections.acquire(self.operation_timeout)?;
            let _global_stream = self.streams.acquire(self.operation_timeout)?;
            let _connection_metric = self
                .metrics
                .as_ref()
                .map(|metrics| KvGaugeGuard::new(metrics, KvGauge::ReaderConnection));
            let _stream_metric = self
                .metrics
                .as_ref()
                .map(|metrics| KvGaugeGuard::new(metrics, KvGauge::ActiveStream));
            let (handle, now_ms) = self.open_handle(binding)?;
            let _resource_stream = handle.streams.acquire(self.operation_timeout)?;
            KvNamespaceRepository::new(self.storage.db())
                .record_open(binding.resource.id, now_ms)?;
            let sink = RefCell::new(sink);
            let result = handle.engine.stream_get(
                key,
                now_ms,
                |entry| (sink.borrow_mut())(KvStreamPart::Entry(entry)),
                |bytes| (sink.borrow_mut())(KvStreamPart::Bytes(bytes.to_vec())),
            );
            handle.touch(self.effective_now_ms());
            result
        })();
        if let (Some(metrics), Err(error)) = (&self.metrics, &result) {
            metrics.observe_product_error(OperationClass::Kv, error.code());
        }
        self.isolate_failure(binding, &result);
        result
    }
}

fn invalid_options() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvInvalidOptions,
        "KV options are outside the supported range",
    )
}

fn cursor_invalid() -> PlatformError {
    PlatformError::new(ErrorCode::KvCursorInvalid, "KV list cursor is invalid")
}

#[cfg(test)]
#[path = "kv_backend_tests.rs"]
mod tests;
