//! Instance-owned Browser Run session admission and ephemeral native CDP state.

mod actions;
mod admission;
mod backend;
mod devtools;
pub(crate) mod http;
mod json;
pub(crate) mod live;
mod websocket;

use crate::metrics::{BrowserOperation, BrowserOutcome, MetricsRegistry};
use backend::BrowserBackend;
use open_compute_core::{BrowserConfig, ErrorCode, InstanceId, PlatformError};
use open_compute_runtime::browser::{
    BrowserCdp, BrowserCdpEvents, BrowserManager, ManagedBrowserSession,
};
use open_compute_storage::PlatformStorage;
use open_compute_storage::browser::{
    BrowserSessionCloseReason, BrowserSessionRecord, BrowserSessionState, BrowserSessions,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct Session {
    generation: String,
    cdp: BrowserCdp,
    managed: Option<Arc<ManagedBrowserSession>>,
    activity: Mutex<Instant>,
    connections: Mutex<BTreeMap<String, i64>>,
    contract: [u8; 32],
    inflight: AtomicUsize,
    keep_alive: Duration,
    is_action: AtomicBool,
    _capacity: OwnedSemaphorePermit,
}

/// Browser lifecycle owner for one registered instance; SQLite owns session visibility/state.
pub struct BrowserService {
    storage: Arc<PlatformStorage>,
    instance: InstanceId,
    config: BrowserConfig,
    manager: Option<Arc<BrowserManager>>,
    generation: String,
    sessions: Mutex<BTreeMap<String, Arc<Session>>>,
    capacity: Arc<Semaphore>,
    pending: Arc<Semaphore>,
    connections: Arc<Semaphore>,
    frontend_capacity: Semaphore,
    frontend_transport: Option<crate::operator_http::OperatorHttpClient>,
    stopped: AtomicBool,
    action_capacity: Arc<Semaphore>,
    active_actions: Mutex<BTreeSet<String>>,
    transport: Option<crate::runtime_bridge::WorkerdTransport>,
    ai: open_compute_core::AiConfig,
    ai_capacity: Arc<Semaphore>,
    live_key: zeroize::Zeroizing<[u8; 32]>,
    control_port: Option<u16>,
    public_origin: Option<url::Url>,
    metrics: Arc<MetricsRegistry>,
}

impl std::fmt::Debug for BrowserService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserService")
            .field("instance", &self.instance)
            .field("stopped", &self.stopped.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl BrowserService {
    /// Validate capacity and reconcile ephemeral sessions; the bound control address owns viewer URLs.
    pub fn new(
        storage: Arc<PlatformStorage>,
        config: BrowserConfig,
        transport: Option<crate::runtime_bridge::WorkerdTransport>,
        ai: open_compute_core::AiConfig,
        control_address: Option<std::net::SocketAddr>,
        metrics: Arc<MetricsRegistry>,
    ) -> Result<Arc<Self>, PlatformError> {
        config.validate()?;
        ai.validate()?;
        let public_origin = config
            .public_origin
            .as_ref()
            .map(|origin| url::Url::parse(origin).map_err(|_| invalid()))
            .transpose()?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| backend::unavailable())?;
        let instance = storage.identity().instance_id;
        let manager = if matches!(
            config.backend,
            open_compute_core::BrowserBackendConfig::Managed { .. }
        ) {
            Some(BrowserManager::new(
                config.clone(),
                storage.data_dir().runtime_dir().join("browser"),
            )?)
        } else {
            None
        };
        BrowserSessions::new(storage.db()).lose_all(instance, now_ms())?;
        let frontend_transport = matches!(
            config.backend,
            open_compute_core::BrowserBackendConfig::Cdp { .. }
        )
        .then(crate::operator_http::OperatorHttpClient::from_process_env)
        .transpose()?;
        let service = Arc::new(Self {
            metrics,
            instance,
            transport,
            ai_capacity: Arc::new(Semaphore::new(usize::from(ai.max_provider_in_flight))),
            ai,
            live_key: zeroize::Zeroizing::new(rand::random()),
            public_origin,
            control_port: control_address
                .filter(|address| address.ip().is_loopback() && address.port() != 0)
                .map(|address| address.port()),
            action_capacity: Arc::new(Semaphore::new(config.max_actions as usize)),
            active_actions: Mutex::new(BTreeSet::new()),
            capacity: Arc::new(Semaphore::new(config.max_sessions as usize)),
            pending: Arc::new(Semaphore::new(config.max_pending_acquires as usize)),
            connections: Arc::new(Semaphore::new(config.max_connections as usize)),
            frontend_capacity: Semaphore::new(config.max_frontend_requests as usize),
            frontend_transport,
            storage,
            manager,
            config,
            generation: uuid::Uuid::now_v7().to_string(),
            sessions: Mutex::new(BTreeMap::new()),
            stopped: AtomicBool::new(false),
        });
        service.prune_history()?;
        let weak = Arc::downgrade(&service);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;
                let Some(service) = weak.upgrade() else {
                    break;
                };
                if service.stopped.load(Ordering::Acquire) {
                    break;
                }
                let _ = service.reconcile().await;
            }
        });
        Ok(service)
    }

    /// Static capability admission, without starting a browser or prewarming external CDP.
    #[must_use]
    pub fn is_available(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
    }

    pub(crate) fn instance_id(&self) -> InstanceId {
        self.instance
    }

    pub(crate) async fn handle_transport(
        self: &Arc<Self>,
        request: axum::extract::Request,
    ) -> axum::response::Response {
        http::dispatch(self.clone(), request)
            .await
            .unwrap_or_else(|error| http::error(&error))
    }

    /// Explicit bounds used by HTTP/WebSocket transports.
    #[must_use]
    pub const fn config(&self) -> &BrowserConfig {
        &self.config
    }

    /// Serve an authenticated immutable Worker binding request on the private listener.
    pub(crate) async fn handle(
        self: &Arc<Self>,
        request: axum::extract::Request,
    ) -> axum::response::Response {
        match http::authorize(self, request.headers()) {
            Ok(()) => http::dispatch(self.clone(), request)
                .await
                .unwrap_or_else(|error| http::error(&error)),
            Err(error) => http::error(&error),
        }
    }

    fn session(&self, id: &str) -> Result<Arc<Session>, PlatformError> {
        if uuid::Uuid::parse_str(id).is_err() || self.stopped.load(Ordering::Acquire) {
            return Err(not_found());
        }
        let sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        let session = sessions.get(id).cloned().ok_or_else(not_found)?;
        let record = BrowserSessions::new(self.storage.db())
            .get(self.instance, id, &session.generation)?
            .ok_or_else(not_found)?;
        if !matches!(
            record.state,
            BrowserSessionState::Ready | BrowserSessionState::Connected
        ) || (!session.cdp.is_alive() && session.inflight.load(Ordering::Acquire) == 0)
        {
            return Err(not_found());
        }
        if session.inflight.load(Ordering::Acquire) == 0
            && session
                .activity
                .lock()
                .map_err(|_| backend::unavailable())?
                .elapsed()
                >= session.keep_alive
        {
            return Err(not_found());
        }
        Ok(session)
    }

    /// Revalidate generation/SQLite authority and refresh activity only for actual CDP commands.
    pub(super) async fn command(
        &self,
        id: &str,
        cdp: &BrowserCdp,
        request: Value,
        current_target: Option<&str>,
    ) -> Result<Value, PlatformError> {
        let observation = self.metrics.browser_operation(BrowserOperation::Command);
        let result = async {
            let client_id = request
                .get("id")
                .and_then(Value::as_i64)
                .filter(|id| (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(id))
                .ok_or_else(invalid)?;
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .filter(|method| {
                    method.len() <= 128
                        && method
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'.')
                })
                .ok_or_else(invalid)?;
            let mut params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            if !params.is_object()
                || request.as_object().is_none_or(|fields| {
                    fields
                        .keys()
                        .any(|key| !matches!(key.as_str(), "id" | "method" | "params" | "sessionId"))
                })
            {
                return Err(invalid());
            }
            let attachment = request
                .get("sessionId")
                .map(|v| v.as_str().filter(|s| s.len() <= 256).ok_or_else(invalid))
                .transpose()?;
            let session = {
                let sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
                let session = sessions.get(id).cloned().ok_or_else(not_found)?;
                if self.stopped.load(Ordering::Acquire) || !session.cdp.is_alive() {
                    return Err(not_found());
                }
                let mut activity = session
                    .activity
                    .lock()
                    .map_err(|_| backend::unavailable())?;
                if session.inflight.load(Ordering::Acquire) == 0
                    && activity.elapsed() >= session.keep_alive
                {
                    return Err(not_found());
                }
                BrowserSessions::new(self.storage.db()).activity(
                    self.instance,
                    id,
                    &session.generation,
                    now_ms(),
                )?;
                session.inflight.fetch_add(1, Ordering::AcqRel);
                *activity = Instant::now();
                drop(activity);
                session
            };
            let _lease = CommandLease(session.clone());
            if method == "Target.createBrowserContext" && session.is_action.load(Ordering::Acquire) {
                // Native connection ownership also disposes contexts whose creation reply is lost.
                params["disposeOnDetach"] = true.into();
            }
            let mut result = if method == "Cloudflare.getLiveView" {
                json!({"result":self.cdp_live_view(id, cdp, params, attachment, current_target).await?})
            } else {
                cdp.command(method, params, attachment)
                    .await
                    .map_err(|_| backend::unavailable())?
            };
            if result.get("error").is_some() {
                result = json!({"error":{"code":-32000,"message":"Browser command failed"}});
            }
            if method == "Browser.close" && result.get("error").is_none() {
                self.close(id, false).await?;
            }
            let fields = result.as_object_mut().ok_or_else(backend::unavailable)?;
            fields.insert("id".into(), client_id.into());
            if let Some(attachment) = attachment {
                fields.insert("sessionId".into(), attachment.into());
            }
            Ok(result)
        }.await;
        observation.finish(match &result {
            Ok(value) if value.get("error").is_some() => BrowserOutcome::Failure,
            _ => BrowserOutcome::result(&result),
        });
        result
    }

    /// Attach one connection lease; transport keepalive/ping never refreshes session activity.
    pub(super) async fn attach(
        self: &Arc<Self>,
        id: &str,
        target: Option<&str>,
    ) -> Result<(BrowserConnection, BrowserCdpEvents), PlatformError> {
        let observation = self.metrics.browser_operation(BrowserOperation::Connect);
        let result = async {
            let capacity = self
                .connections
                .clone()
                .try_acquire_owned()
                .map_err(|_| limit())?;
            let session = self.session(id)?;
            if let Some(target) = target {
                if !devtools::valid_target(target) {
                    return Err(not_found());
                }
                let reply = self
                    .command(
                        id,
                        &session.cdp,
                        json!({"id":0,"method":"Target.getTargetInfo","params":{"targetId":target}}),
                        None,
                    )
                    .await?;
                if reply
                    .pointer("/result/targetInfo/targetId")
                    .and_then(Value::as_str)
                    != Some(target)
                {
                    return Err(not_found());
                }
            }

            let (cdp, contract) = if let Some(managed) = &session.managed {
                (managed.connect(target).await?, session.contract)
            } else {
                let backend = tokio::time::timeout(
                    Duration::from_millis(self.config.acquire_timeout_ms),
                    BrowserBackend::connect(&self.config, target),
                )
                .await
                .map_err(|_| timeout())??;
                (backend.cdp, backend.contract)
            };
            if contract != session.contract {
                return Err(backend::unavailable());
            }
            let sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
            if self.stopped.load(Ordering::Acquire)
                || !sessions
                    .get(id)
                    .is_some_and(|current| Arc::ptr_eq(current, &session))
            {
                return Err(not_found());
            }
            let connection = uuid::Uuid::now_v7().to_string();
            let connected_at_ms = now_ms();
            let mut connections = session
                .connections
                .lock()
                .map_err(|_| backend::unavailable())?;
            BrowserSessions::new(self.storage.db()).connection(
                self.instance,
                id,
                &session.generation,
                true,
                connected_at_ms,
            )?;
            connections.insert(connection.clone(), connected_at_ms);
            drop(connections);
            drop(sessions);
            let events = cdp.subscribe();
            Ok((
                BrowserConnection {
                    service: self.clone(),
                    session,
                    id: id.into(),
                    connection,
                    cdp,
                    target: target.map(str::to_owned),
                    _capacity: capacity,
                },
                events,
            ))
        }.await;
        observation.finish(BrowserOutcome::result(&result));
        result
    }

    /// Close local session authority idempotently; idle expiry never closes an external browser.
    pub async fn close(&self, id: &str, lost: bool) -> Result<(), PlatformError> {
        let observation = self.metrics.browser_operation(BrowserOperation::Close);
        let result = async {
            let managed = self
                .sessions
                .lock()
                .map_err(|_| backend::unavailable())?
                .get(id)
                .and_then(|session| session.managed.clone());
            if let Some(managed) = managed {
                let cleanup = managed.close().await;
                if !lost {
                    cleanup?;
                }
            }
            self.close_now(
                id,
                if lost {
                    BrowserSessionCloseReason::Lost
                } else {
                    BrowserSessionCloseReason::Normal
                },
            )
        }
        .await;
        observation.finish(BrowserOutcome::result(&result));
        result
    }

    pub(crate) async fn close_browser(&self, id: &str) -> Result<(), PlatformError> {
        let session = self.session(id)?;
        let reply = self
            .command(
                id,
                &session.cdp,
                json!({"id":0,"method":"Browser.close"}),
                None,
            )
            .await?;
        if reply.get("error").is_some() {
            return Err(backend::unavailable());
        }
        Ok(())
    }

    fn close_now(&self, id: &str, reason: BrowserSessionCloseReason) -> Result<(), PlatformError> {
        let mut sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        if let Some(session) = sessions.get(id) {
            let store = BrowserSessions::new(self.storage.db());
            store.begin_close(self.instance, id, &session.generation)?;
            store.finish_close(self.instance, id, &session.generation, reason, now_ms())?;
            sessions.remove(id);
            self.metrics.set_browser_sessions(sessions.len() as u64);
        }
        drop(sessions);
        self.prune_history()
    }

    /// Project browser and viewer endpoints from operator authority, never request headers.
    pub(crate) fn public_urls(
        &self,
        session: &str,
        listener_port: Option<u16>,
    ) -> Result<(String, String), PlatformError> {
        let account = self.instance;
        let mut websocket = match &self.public_origin {
            Some(origin) => origin.clone(),
            None => {
                let port = listener_port
                    .or(self.control_port)
                    .ok_or_else(http::unsupported)?;
                url::Url::parse(&format!("http://{account}.localhost:{port}"))
                    .map_err(|_| invalid())?
            }
        };
        let live = websocket
            .join(&format!(
                "/client/v4/accounts/{account}/browser-rendering/live/"
            ))
            .map_err(|_| invalid())?;
        websocket
            .set_scheme(if websocket.scheme() == "https" {
                "wss"
            } else {
                "ws"
            })
            .map_err(|()| invalid())?;
        websocket.set_path(&format!(
            "/client/v4/accounts/{account}/browser-rendering/devtools/browser/{session}"
        ));
        Ok((websocket.into(), live.into()))
    }

    /// Stable instance-scoped session/history discovery; raw CDP endpoint/locator stays private.
    pub fn list(&self, history: bool, limit: u32, offset: u32) -> Result<Value, PlatformError> {
        let records =
            BrowserSessions::new(self.storage.db()).list(self.instance, history, limit, offset)?;
        let sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        let rows = records
            .into_iter()
            .map(|record| {
                let mut value = json!({"sessionId":record.id,"startTime":record.created_at_ms});
                if let Some(session) = sessions.get(&record.id)
                    && let Some(connection) = session.connections.lock().ok().and_then(|c| {
                        c.last_key_value()
                            .map(|(id, started)| (id.clone(), *started))
                    })
                {
                    value["connectionId"] = connection.0.into();
                    value["connectionStartTime"] = connection.1.to_string().into();
                }
                if history {
                    value["endTime"] = record.closed_at_ms.ok_or_else(backend::unavailable)?.into();
                    match record.close_reason.ok_or_else(backend::unavailable)? {
                        BrowserSessionCloseReason::Normal => {
                            value["closeReason"] = 1.into();
                            value["closeReasonText"] = "NormalClosure".into();
                        }
                        BrowserSessionCloseReason::Idle => {
                            value["closeReason"] = 2.into();
                            value["closeReasonText"] = "BrowserIdle".into();
                        }
                        BrowserSessionCloseReason::Lost => {
                            // Cloudflare's GraphQL schema defines 0 as Unknown; loss does not prove a specific cause.
                            value["closeReason"] = 0.into();
                            value["closeReasonText"] = "Unknown".into();
                        }
                    }
                }
                Ok(value)
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        Ok(if history {
            json!({"history":rows})
        } else {
            json!({"sessions":rows})
        })
    }

    /// Fixed client limits report effective local capacity rather than a hosted plan limit.
    pub fn limits(&self) -> Result<Value, PlatformError> {
        let sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        let active = sessions
            .keys()
            .map(|id| json!({"id":id}))
            .collect::<Vec<_>>();
        Ok(
            json!({"activeSessions":active,"maxConcurrentSessions":self.config.max_sessions,
            "allowedBrowserAcquisitions":usize::from(self.capacity.available_permits()>0),"timeUntilNextAllowedBrowserAcquisition":0}),
        )
    }

    async fn reconcile(&self) -> Result<(), PlatformError> {
        self.reconcile_sessions()?;
        if let Some(manager) = &self.manager {
            manager.reconcile().await?;
        }
        Ok(())
    }

    fn reconcile_sessions(&self) -> Result<(), PlatformError> {
        let mut sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        let mut expired = Vec::new();
        for (id, session) in sessions.iter() {
            let inflight = session.inflight.load(Ordering::Acquire);
            // Command completion owns its close result; losing the anchor must not race Browser.close.
            let lost = inflight == 0 && !session.cdp.is_alive();
            let idle = inflight == 0
                && session
                    .activity
                    .lock()
                    .map_err(|_| backend::unavailable())?
                    .elapsed()
                    >= session.keep_alive;
            if lost || idle {
                expired.push((
                    id.clone(),
                    if lost {
                        BrowserSessionCloseReason::Lost
                    } else {
                        BrowserSessionCloseReason::Idle
                    },
                ));
            }
        }
        for (id, reason) in expired {
            let generation = &sessions.get(&id).ok_or_else(not_found)?.generation;
            let store = BrowserSessions::new(self.storage.db());
            store.begin_close(self.instance, &id, generation)?;
            store.finish_close(self.instance, &id, generation, reason, now_ms())?;
            sessions.remove(&id);
            self.metrics.set_browser_sessions(sessions.len() as u64);
        }
        drop(sessions);
        self.prune_history()
    }

    fn prune_history(&self) -> Result<(), PlatformError> {
        let retention =
            i64::try_from(self.config.history_retention_ms).map_err(|_| backend::unavailable())?;
        BrowserSessions::new(self.storage.db()).retain_history(
            self.instance,
            now_ms().saturating_sub(retention),
            self.config.max_history_entries,
        )?;
        Ok(())
    }

    /// Fence all transports when their invoking workerd generation exits.
    pub(crate) async fn invalidate_sessions(&self) -> Result<(), PlatformError> {
        let ids = self
            .sessions
            .lock()
            .map_err(|_| backend::unavailable())?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            self.close(&id, true).await?;
        }
        Ok(())
    }

    /// Stop admission, invalidate generation-local sessions and release native CDP transports.
    pub async fn shutdown(&self) -> Result<(), PlatformError> {
        self.stopped.store(true, Ordering::Release);
        self.capacity.close();
        let mut failure = None;
        let ids = match self.sessions.lock() {
            Ok(sessions) => sessions.keys().cloned().collect::<Vec<_>>(),
            Err(_) => {
                failure = Some(backend::unavailable());
                Vec::new()
            }
        };
        for id in ids {
            if let Err(error) = self.close(&id, true).await {
                failure.get_or_insert(error);
            }
        }
        if let Some(manager) = &self.manager
            && let Err(error) = manager.shutdown().await
        {
            failure.get_or_insert(error);
        }
        failure.map_or(Ok(()), Err)
    }
}

struct CommandLease(Arc<Session>);
impl Drop for CommandLease {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(super) struct BrowserConnection {
    service: Arc<BrowserService>,
    session: Arc<Session>,
    id: String,
    connection: String,
    cdp: BrowserCdp,
    target: Option<String>,
    _capacity: OwnedSemaphorePermit,
}
impl Drop for BrowserConnection {
    fn drop(&mut self) {
        if let Ok(mut connections) = self.session.connections.lock()
            && connections.remove(&self.connection).is_some()
        {
            let _ = BrowserSessions::new(self.service.storage.db()).connection(
                self.service.instance,
                &self.id,
                &self.session.generation,
                false,
                now_ms(),
            );
        }
    }
}
fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}
pub(crate) fn invalid() -> PlatformError {
    PlatformError::new(ErrorCode::BrowserInputInvalid, "browser request is invalid")
}
pub(crate) fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserSessionNotFound,
        "browser session is unavailable",
    )
}
pub(crate) fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserLimitExceeded,
        "browser capacity is exhausted",
    )
}
fn timeout() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserTimeout,
        "browser request deadline elapsed",
    )
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
