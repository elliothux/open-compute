//! Workers Logs ingestion, persistence, and process-local realtime tail authority.

use crate::metrics::MetricsRegistry;
use crate::observability_filter::{Combination, FilterNode};
use base64::Engine as _;
use hmac::{Hmac, Mac as _};
use open_compute_core::config::ObservabilityConfig;
use open_compute_core::{AccountId, PlatformError, SecretString, VersionId, WorkerId};
use open_compute_storage::{
    NewObservabilityInvocation, ObservabilityEventCursor, ObservabilityStore, PlatformStorage,
    WorkerObservabilitySettings, WorkerRecord, WorkerRepository,
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, interval, timeout_at};

#[path = "observability_session.rs"]
mod session;

#[path = "observability_model.rs"]
mod model;

pub(crate) use model::workers_logs_dataset;
use model::{
    canonical_invocation, constant_time_equal, enqueue_overload, format_timestamp, invalid,
    live_event, loader_identity, matches_tail, not_found, now_ms, sampled, stale, ticket_claim,
    unavailable, validate_filters,
};

const MAX_INGEST_BYTES: usize = 256 * 1024;
const MAX_BATCH_ITEMS: usize = 128;
const MAX_FILTERS: usize = 8;
const MAX_FILTER_VALUES: usize = 16;
const MAX_FILTER_TEXT: usize = 512;
const MAX_DROP_STREAK: u32 = 64;
const LIVE_TAIL_ELIGIBILITY_MS: u64 = 45_000;
const DATASET: &str = "cloudflare-workers";

type HmacSha256 = Hmac<Sha256>;

/// A validated fixed-Wrangler Script Tail filter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TailFilter {
    /// Stable per-session invocation sampling.
    Sampling(f64),
    /// Workerd outcome allowlist.
    Outcome(Vec<String>),
    /// HTTP method allowlist.
    Method(Vec<String>),
    /// Redacted request-header substring match.
    Header { key: String, query: String },
    /// Trusted ingress client addresses.
    ClientIp(Vec<IpAddr>),
    /// Console message substring.
    Query(String),
    /// Exact immutable Cloudflare Version identity.
    ScriptVersion(VersionId),
}

/// Script Tail object returned by the official REST endpoints.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TailView {
    /// Opaque process-local session identity.
    pub id: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
    /// Signed opaque WebSocket URL.
    pub url: String,
}

/// One byte-accounted frame delivered to a connected Script Tail.
pub(crate) struct TailFrame {
    pub(crate) text: String,
    pub(crate) bytes: usize,
    queued_bytes: Arc<AtomicUsize>,
}

impl Drop for TailFrame {
    fn drop(&mut self) {
        self.queued_bytes.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// Result of authenticating and attaching one WebSocket connection.
pub(crate) struct TailConnection {
    pub(crate) receiver: mpsc::Receiver<TailFrame>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorClaims {
    schema_version: u8,
    account_id: String,
    query_id: String,
    from_ms: i64,
    to_ms: i64,
    timestamp_ms: i64,
    event_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectorEnvelope {
    schema_version: u8,
    collector_event_id: String,
    identity: CollectorIdentity,
    items: Vec<Value>,
    #[serde(default)]
    batch_truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectorIdentity {
    schema_version: u8,
    account_id: String,
    worker_id: String,
    script_name: String,
    version_id: String,
    deployment_id: Option<String>,
    route_generation: u64,
    observability_generation: u64,
    enabled: bool,
    logs_enabled: bool,
    head_sampling_rate: f64,
    invocation_logs: bool,
    persist: bool,
}

#[derive(Clone)]
struct EffectiveIdentity {
    account_id: AccountId,
    worker: WorkerRecord,
    version_id: VersionId,
    deployment_id: Option<String>,
    settings: WorkerObservabilitySettings,
    secret_values: Arc<Vec<SecretString>>,
}

struct TailSession {
    id: String,
    account_id: AccountId,
    worker_id: WorkerId,
    expires_at_ms: i64,
    ticket: String,
    protocol: TailProtocol,
    connected: bool,
    sender: Option<mpsc::Sender<TailFrame>>,
    queued_bytes: Arc<AtomicUsize>,
    overloaded: bool,
    drop_streak: u32,
}

enum TailProtocol {
    Script(Vec<TailFilter>),
    Live {
        combination: Combination,
        filters: Vec<FilterNode>,
    },
}

/// WebSocket location returned by Telemetry Live Tail prepare.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveTailView {
    /// Opaque signed WebSocket location.
    pub(crate) ws_url: String,
}

/// One process owns all Workers Logs and Script Tail state.
pub(crate) struct ObservabilityService {
    storage: Arc<PlatformStorage>,
    store: Option<Arc<ObservabilityStore>>,
    config: ObservabilityConfig,
    signing_key: [u8; 32],
    sessions: Mutex<HashMap<String, TailSession>>,
    persistence: mpsc::Sender<NewObservabilityInvocation>,
    metrics: Arc<MetricsRegistry>,
}

impl std::fmt::Debug for ObservabilityService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservabilityService")
            .field("store_available", &self.store.is_some())
            .field("session_count", &self.session_count())
            .finish_non_exhaustive()
    }
}

impl ObservabilityService {
    /// Construct the bounded ingest queue and its single SQLite writer.
    #[must_use]
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        store: Option<Arc<ObservabilityStore>>,
        config: ObservabilityConfig,
        metrics: Arc<MetricsRegistry>,
    ) -> Arc<Self> {
        let capacity = usize::try_from(config.ingest_queue_events)
            .unwrap_or(1)
            .max(1);
        let (persistence, receiver) = mpsc::channel(capacity);
        let mut signing_key = [0_u8; 32];
        rand::rng().fill_bytes(&mut signing_key);
        let service = Arc::new(Self {
            storage,
            store,
            config,
            signing_key,
            sessions: Mutex::new(HashMap::new()),
            persistence,
            metrics,
        });
        service.spawn_writer(receiver);
        service
    }

    fn spawn_writer(self: &Arc<Self>, mut receiver: mpsc::Receiver<NewObservabilityInvocation>) {
        let store = self.store.clone();
        let storage = Arc::downgrade(&self.storage);
        let metrics = self.metrics.clone();
        let batch_limit = usize::try_from(self.config.ingest_batch_events)
            .unwrap_or(1)
            .max(1);
        let flush_delay = Duration::from_millis(self.config.ingest_flush_ms);
        let maintenance_period =
            Duration::from_millis(self.config.retention_ms.clamp(1_000, 60_000));
        tokio::spawn(async move {
            let mut maintenance = interval(maintenance_period);
            maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            maintenance.tick().await;
            loop {
                let first = tokio::select! {
                    value = receiver.recv() => value,
                    _ = maintenance.tick() => {
                        let Some(store) = store.clone() else { continue; };
                        let Ok(now) = now_ms() else { continue; };
                        let maintenance_store = store.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            maintenance_store.prune(now, 100_000)
                        }).await;
                        if matches!(result, Ok(Err(_)) | Err(_)) {
                            tracing::warn!("Workers Logs retention maintenance failed");
                        }
                        if let (Ok(bytes), Ok(oldest)) =
                            (store.accounted_bytes(), store.oldest_event_ms())
                        {
                            let age_ms = oldest.map_or(0, |value| now.saturating_sub(value));
                            metrics.set_observability_storage(
                                bytes,
                                Duration::from_millis(u64::try_from(age_ms).unwrap_or(0)),
                            );
                        }
                        continue;
                    }
                };
                let Some(first) = first else {
                    break;
                };
                let Some(store) = store.clone() else {
                    continue;
                };
                let mut batch = Vec::with_capacity(batch_limit);
                batch.push(first);
                let deadline = Instant::now() + flush_delay;
                while batch.len() < batch_limit {
                    match timeout_at(deadline, receiver.recv()).await {
                        Ok(Some(invocation)) => batch.push(invocation),
                        Ok(None) | Err(_) => break,
                    }
                }
                let Some(write_storage) = storage.upgrade() else {
                    break;
                };
                let write_store = store.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let encoded_bytes =
                        batch.iter().try_fold(0_u64, |total, invocation| {
                            let encoded = serde_json::to_vec(invocation).map_err(|_| invalid())?;
                            Ok::<_, PlatformError>(total.saturating_add(
                                u64::try_from(encoded.len()).map_err(|_| invalid())?,
                            ))
                        })?;
                    let reserved_bytes = encoded_bytes
                        .saturating_mul(2)
                        .saturating_add(64 * 1024)
                        .max(1);
                    let _reservation = write_storage.reserve_mutation(reserved_bytes)?;
                    write_store.insert_batch(&batch)
                })
                .await;
                metrics.set_observability_ingest_queue_depth(
                    receiver.max_capacity().saturating_sub(receiver.capacity()),
                );
                if matches!(&result, Ok(Ok(_)))
                    && let (Ok(bytes), Ok(oldest), Ok(now)) =
                        (store.accounted_bytes(), store.oldest_event_ms(), now_ms())
                {
                    let age_ms = oldest.map_or(0, |value| now.saturating_sub(value));
                    metrics.set_observability_storage(
                        bytes,
                        Duration::from_millis(u64::try_from(age_ms).unwrap_or(0)),
                    );
                }
                if let Ok(Err(error)) = result {
                    tracing::warn!(
                        code = error.code().as_str(),
                        "Workers Logs persistence dropped an invocation batch"
                    );
                } else if result.is_err() {
                    tracing::warn!("Workers Logs persistence task failed");
                }
            }
        });
    }

    /// Whether the independent Workers Logs database passed startup validation.
    #[must_use]
    pub(crate) fn store(&self) -> Option<&Arc<ObservabilityStore>> {
        self.store.as_ref()
    }

    /// Borrow the immutable installation-local observability limits.
    #[must_use]
    pub(crate) const fn config(&self) -> &ObservabilityConfig {
        &self.config
    }

    /// Record a collector request after generation authentication.
    pub(crate) fn observe_ingest_result(&self, success: bool) {
        self.metrics.observe_observability_ingest(success);
    }

    /// Record one public Telemetry query without tenant-valued labels.
    pub(crate) fn observe_query(&self, invocations: bool, success: bool, duration: Duration) {
        self.metrics
            .observe_observability_query(invocations, success, duration);
    }

    /// Sign an event pagination boundary to its account, query, and timeframe.
    pub(crate) fn encode_cursor(
        &self,
        account_id: AccountId,
        query_id: &str,
        from_ms: i64,
        to_ms: i64,
        cursor: &ObservabilityEventCursor,
    ) -> Result<String, PlatformError> {
        let claims = CursorClaims {
            schema_version: 1,
            account_id: account_id.to_string(),
            query_id: query_id.to_owned(),
            from_ms,
            to_ms,
            timestamp_ms: cursor.timestamp_ms,
            event_id: cursor.event_id.clone(),
        };
        let payload = serde_json::to_vec(&claims).map_err(|_| invalid())?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key).map_err(|_| unavailable())?;
        mac.update(&payload);
        let signature = mac.finalize().into_bytes();
        Ok(format!(
            "{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    /// Verify and decode one event pagination boundary.
    pub(crate) fn decode_cursor(
        &self,
        encoded: &str,
        account_id: AccountId,
        query_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<ObservabilityEventCursor, PlatformError> {
        let (payload, signature) = encoded.split_once('.').ok_or_else(invalid)?;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| invalid())?;
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| invalid())?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key).map_err(|_| unavailable())?;
        mac.update(&payload);
        mac.verify_slice(&signature).map_err(|_| invalid())?;
        let claims: CursorClaims = serde_json::from_slice(&payload).map_err(|_| invalid())?;
        if claims.schema_version != 1
            || claims.account_id != account_id.to_string()
            || claims.query_id != query_id
            || claims.from_ms != from_ms
            || claims.to_ms != to_ms
            || claims.event_id.is_empty()
        {
            return Err(invalid());
        }
        Ok(ObservabilityEventCursor {
            timestamp_ms: claims.timestamp_ms,
            event_id: claims.event_id,
        })
    }

    /// Current process-local Script Tail count after expiry collection.
    pub(crate) fn session_count(&self) -> usize {
        let now = now_ms().unwrap_or(i64::MAX);
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|_, session| session.expires_at_ms > now);
        let count = sessions.len();
        drop(sessions);
        self.metrics.set_observability_tail_sessions(count);
        count
    }

    /// Return closed-client and overload drop counters without tenant labels.
    pub(crate) fn tail_drop_counts(&self) -> [u64; 2] {
        self.metrics.observability_tail_dropped()
    }

    /// Validate, canonicalize, fan out, and enqueue one collector envelope.
    pub(crate) fn ingest(&self, bytes: &[u8]) -> Result<(), PlatformError> {
        if bytes.is_empty() || bytes.len() > MAX_INGEST_BYTES {
            return Err(invalid());
        }
        let envelope: CollectorEnvelope = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if envelope.schema_version != 1
            || envelope.collector_event_id.is_empty()
            || envelope.collector_event_id.len() > 128
            || envelope.items.is_empty()
            || envelope.items.len() > MAX_BATCH_ITEMS
        {
            return Err(invalid());
        }
        let collector = self.authorize_collector(&envelope.identity)?;
        let received_at_ms = now_ms()?;
        if envelope.batch_truncated {
            self.metrics.inc_observability_truncated(false);
        }
        let mut accepted = 0_usize;
        for (index, item) in envelope.items.into_iter().enumerate() {
            let Ok(identity) = self.identity_for_item(&collector, &item) else {
                self.metrics.observe_observability_event(0, false);
                continue;
            };
            let Ok(invocation) = canonical_invocation(
                &envelope.collector_event_id,
                index,
                item,
                &identity,
                received_at_ms,
                envelope.batch_truncated,
                self.config.max_invocation_log_bytes,
            ) else {
                self.metrics.observe_observability_event(0, false);
                continue;
            };
            accepted = accepted.saturating_add(1);
            if invocation.truncated {
                self.metrics.inc_observability_truncated(true);
            }
            for event in &invocation.events {
                let kind = if event.metadata_type == "cf-worker-event" {
                    0
                } else if event.level.as_deref() == Some("error") {
                    2
                } else {
                    1
                };
                self.metrics.observe_observability_event(kind, true);
            }
            self.fan_out(&identity, &invocation);
            if identity.settings.enabled
                && identity.settings.logs_enabled
                && identity.settings.persist
                && !invocation.events.is_empty()
                && sampled(
                    &invocation.invocation_id,
                    &format!("persistence/{}", identity.settings.generation),
                    identity.settings.effective_head_sampling_rate(),
                )
            {
                if self.persistence.try_send(invocation).is_err() {
                    self.metrics.observe_observability_event(0, false);
                }
                self.metrics.set_observability_ingest_queue_depth(
                    self.persistence
                        .max_capacity()
                        .saturating_sub(self.persistence.capacity()),
                );
            }
        }
        if accepted == 0 {
            Err(invalid())
        } else {
            Ok(())
        }
    }

    fn authorize_collector(
        &self,
        identity: &CollectorIdentity,
    ) -> Result<EffectiveIdentity, PlatformError> {
        if identity.schema_version != 1
            || identity.script_name.is_empty()
            || identity.script_name.len() > 63
            || !identity.head_sampling_rate.is_finite()
            || !(0.0..=1.0).contains(&identity.head_sampling_rate)
        {
            return Err(invalid());
        }
        let account_id = identity
            .account_id
            .parse::<AccountId>()
            .map_err(|_| invalid())?;
        let worker_id = identity
            .worker_id
            .parse::<WorkerId>()
            .map_err(|_| invalid())?;
        let version_id = identity
            .version_id
            .parse::<VersionId>()
            .map_err(|_| invalid())?;
        let repo = WorkerRepository::new(self.storage.db());
        let worker = repo.get_worker(account_id, worker_id)?;
        let version = repo.get_worker_version(account_id, worker_id, version_id)?;
        let settings = repo.get_observability_settings(account_id, worker_id)?;
        if worker.deleted_at_ms.is_some()
            || version.deleted_at_ms.is_some()
            || worker.name != identity.script_name
            || worker.route_generation != identity.route_generation
            || settings.generation != identity.observability_generation
            || settings.enabled != identity.enabled
            || settings.logs_enabled != identity.logs_enabled
            || settings.invocation_logs != identity.invocation_logs
            || settings.persist != identity.persist
            || (settings.effective_head_sampling_rate() - identity.head_sampling_rate).abs()
                > f64::EPSILON
            || (if worker.active_version_id == Some(version_id) {
                worker.active_deployment_id.map(|value| value.to_string())
            } else {
                None
            }) != identity.deployment_id
        {
            return Err(stale());
        }
        Ok(EffectiveIdentity {
            account_id,
            worker,
            version_id,
            deployment_id: identity.deployment_id.clone(),
            settings,
            secret_values: self.secret_values(repo, account_id, worker_id, version_id)?,
        })
    }

    fn identity_for_item(
        &self,
        collector: &EffectiveIdentity,
        item: &Value,
    ) -> Result<EffectiveIdentity, PlatformError> {
        let runtime_name = match item.get("scriptName") {
            None | Some(Value::Null) => return Ok(collector.clone()),
            Some(Value::String(value)) => value,
            Some(_) => return Err(stale()),
        };
        if runtime_name == collector.worker.name.as_str() {
            return Ok(collector.clone());
        }
        let Some((account_id, worker_id, version_id)) = loader_identity(runtime_name) else {
            return Err(stale());
        };
        if account_id == collector.account_id
            && worker_id == collector.worker.id
            && version_id == collector.version_id
        {
            return Ok(collector.clone());
        }
        Err(stale())
    }

    fn secret_values(
        &self,
        repo: WorkerRepository<'_>,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<Arc<Vec<SecretString>>, PlatformError> {
        let snapshot = repo.version_snapshot(account_id, worker_id, version_id, false)?;
        let mut values = Vec::with_capacity(snapshot.secrets.len());
        for secret in snapshot.secrets.values() {
            let plaintext = self.storage.crypto().decrypt(
                &secret.envelope,
                account_id,
                worker_id,
                version_id,
                &secret.name,
                &secret.revision_id,
            )?;
            let value = std::str::from_utf8(plaintext.expose()).map_err(|_| stale())?;
            if !value.is_empty() {
                values.push(SecretString::new(value));
            }
        }
        Ok(Arc::new(values))
    }
}
