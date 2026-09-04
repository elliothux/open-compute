//! Process-local Script Tail and Dashboard Live Tail session authority.

use super::{
    EffectiveIdentity, HmacSha256, LIVE_TAIL_ELIGIBILITY_MS, LiveTailView, MAX_DROP_STREAK,
    ObservabilityService, TailConnection, TailFilter, TailFrame, TailProtocol, TailSession,
    TailView, constant_time_equal, enqueue_overload, format_timestamp, invalid, live_event,
    matches_tail, not_found, now_ms, ticket_claim, unavailable, validate_filters,
};
use crate::observability_filter::{Combination, FilterNode};
use base64::Engine as _;
use hmac::Mac as _;
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId, WorkerId};
use open_compute_storage::{
    NewObservabilityInvocation, ObservabilityAudit, WorkerRecord, WorkerRepository,
};
use rand::RngCore as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

impl ObservabilityService {
    /// Create one process-local signed Script Tail session.
    pub(crate) fn create_tail(
        &self,
        account_id: AccountId,
        worker: &WorkerRecord,
        filters: Vec<TailFilter>,
        request_id: RequestId,
    ) -> Result<TailView, PlatformError> {
        validate_filters(&filters)?;
        let (id, ticket, expires_at_ms) = self.create_session(
            account_id,
            worker,
            TailProtocol::Script(filters),
            request_id,
        )?;
        Ok(TailView {
            id: id.clone(),
            expires_at: format_timestamp(expires_at_ms)?,
            url: self.tail_url(&id, &ticket)?,
        })
    }

    /// Create one Dashboard Telemetry Live Tail session.
    pub(crate) fn create_live_tail(
        &self,
        account_id: AccountId,
        worker: &WorkerRecord,
        combination: Combination,
        filters: Vec<FilterNode>,
        request_id: RequestId,
    ) -> Result<LiveTailView, PlatformError> {
        crate::observability_filter::validate(&filters)?;
        let (id, ticket, _) = self.create_session(
            account_id,
            worker,
            TailProtocol::Live {
                combination,
                filters,
            },
            request_id,
        )?;
        Ok(LiveTailView {
            ws_url: self.live_tail_url(&id, &ticket)?,
        })
    }

    fn create_session(
        &self,
        account_id: AccountId,
        worker: &WorkerRecord,
        protocol: TailProtocol,
        request_id: RequestId,
    ) -> Result<(String, String, i64), PlatformError> {
        let now = now_ms()?;
        let ttl_ms = match &protocol {
            TailProtocol::Script(_) => self.config.tail_session_ttl_ms,
            TailProtocol::Live { .. } => self
                .config
                .tail_session_ttl_ms
                .min(LIVE_TAIL_ELIGIBILITY_MS),
        };
        let expires_at_ms = now.saturating_add(i64::try_from(ttl_ms).map_err(|_| invalid())?);
        let mut id_bytes = [0_u8; 16];
        rand::rng().fill_bytes(&mut id_bytes);
        let id = hex::encode(id_bytes);
        let ticket = self.sign_ticket(&id, account_id, worker.id, expires_at_ms)?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires_at_ms > now);
        let active = sessions
            .values()
            .filter(|session| session.account_id == account_id && session.worker_id == worker.id)
            .count();
        if active >= usize::from(self.config.max_tail_sessions_per_script) {
            return Err(PlatformError::new(
                ErrorCode::AdmissionBusy,
                "Script Tail client limit was reached",
            ));
        }
        sessions.insert(
            id.clone(),
            TailSession {
                id: id.clone(),
                account_id,
                worker_id: worker.id,
                expires_at_ms,
                ticket: ticket.clone(),
                protocol,
                connected: false,
                sender: None,
                queued_bytes: Arc::new(AtomicUsize::new(0)),
                overloaded: false,
                drop_streak: 0,
            },
        );
        self.metrics.set_observability_tail_sessions(sessions.len());
        drop(sessions);
        if let Err(error) = WorkerRepository::new(self.storage.db()).audit_observability(
            account_id,
            &ObservabilityAudit::TailCreate {
                worker_id: worker.id,
            },
            request_id,
            now,
        ) {
            if let Ok(mut sessions) = self.sessions.lock() {
                sessions.remove(&id);
                self.metrics.set_observability_tail_sessions(sessions.len());
            }
            return Err(error);
        }
        Ok((id, ticket, expires_at_ms))
    }

    /// List active sessions for one account-scoped Script without a separate raw ticket field.
    pub(crate) fn list_tails(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<TailView>, PlatformError> {
        let now = now_ms()?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires_at_ms > now);
        sessions
            .values()
            .filter(|session| {
                session.account_id == account_id
                    && session.worker_id == worker_id
                    && matches!(&session.protocol, TailProtocol::Script(_))
            })
            .map(|session| {
                Ok(TailView {
                    id: session.id.clone(),
                    expires_at: format_timestamp(session.expires_at_ms)?,
                    url: self.tail_url(&session.id, &session.ticket)?,
                })
            })
            .collect()
    }

    /// Delete and revoke one active account-scoped Script Tail session.
    pub(crate) fn delete_tail(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        id: &str,
        request_id: RequestId,
    ) -> Result<(), PlatformError> {
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        match sessions.get(id) {
            Some(session)
                if session.account_id == account_id
                    && session.worker_id == worker_id
                    && matches!(&session.protocol, TailProtocol::Script(_)) => {}
            _ => return Err(not_found()),
        }
        WorkerRepository::new(self.storage.db()).audit_observability(
            account_id,
            &ObservabilityAudit::TailDelete { worker_id },
            request_id,
            now_ms()?,
        )?;
        sessions.remove(id);
        self.metrics.set_observability_tail_sessions(sessions.len());
        Ok(())
    }

    /// Record one content-free successful telemetry query.
    pub(crate) fn audit_query(
        &self,
        account_id: AccountId,
        event: &ObservabilityAudit,
        request_id: RequestId,
    ) -> Result<(), PlatformError> {
        WorkerRepository::new(self.storage.db()).audit_observability(
            account_id,
            event,
            request_id,
            now_ms()?,
        )
    }

    /// Revoke every process-local tail for a Script that has been tombstoned.
    pub(crate) fn revoke_worker_tails(&self, account_id: AccountId, worker_id: WorkerId) {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|_, session| {
            session.account_id != account_id || session.worker_id != worker_id
        });
        self.metrics.set_observability_tail_sessions(sessions.len());
    }

    /// Authenticate a signed ticket and atomically claim the session's one connection.
    pub(crate) fn connect_tail(
        &self,
        id: &str,
        ticket: &str,
    ) -> Result<TailConnection, PlatformError> {
        let now = now_ms()?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        let session = sessions.get_mut(id).ok_or_else(not_found)?;
        if session.expires_at_ms <= now
            || session.connected
            || !matches!(&session.protocol, TailProtocol::Script(_))
            || !constant_time_equal(session.ticket.as_bytes(), ticket.as_bytes())
            || !self.verify_ticket(session, ticket)
        {
            return Err(not_found());
        }
        let queue_capacity = usize::try_from(self.config.tail_client_queue_bytes / 256)
            .unwrap_or(1)
            .clamp(1, 4096);
        let (sender, receiver) = mpsc::channel(queue_capacity);
        session.sender = Some(sender);
        session.connected = true;
        Ok(TailConnection { receiver })
    }

    /// Authenticate and claim one Telemetry Live Tail WebSocket.
    pub(crate) fn connect_live_tail(
        &self,
        id: &str,
        ticket: &str,
    ) -> Result<TailConnection, PlatformError> {
        let now = now_ms()?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        let session = sessions.get_mut(id).ok_or_else(not_found)?;
        if session.expires_at_ms <= now
            || session.connected
            || !matches!(&session.protocol, TailProtocol::Live { .. })
            || !constant_time_equal(session.ticket.as_bytes(), ticket.as_bytes())
            || !self.verify_ticket(session, ticket)
        {
            return Err(not_found());
        }
        let queue_capacity = usize::try_from(self.config.tail_client_queue_bytes / 256)
            .unwrap_or(1)
            .clamp(1, 4096);
        let (sender, receiver) = mpsc::channel(queue_capacity);
        session.sender = Some(sender);
        session.connected = true;
        Ok(TailConnection { receiver })
    }

    /// Extend every connected Live Tail for the account-scoped Script.
    pub(crate) fn heartbeat_live_tail(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<(), PlatformError> {
        let now = now_ms()?;
        let expires_at_ms = now.saturating_add(
            i64::try_from(
                self.config
                    .tail_session_ttl_ms
                    .min(LIVE_TAIL_ELIGIBILITY_MS),
            )
            .map_err(|_| invalid())?,
        );
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires_at_ms > now);
        for session in sessions.values_mut() {
            if session.account_id == account_id
                && session.worker_id == worker_id
                && session.connected
                && matches!(&session.protocol, TailProtocol::Live { .. })
            {
                session.expires_at_ms = expires_at_ms;
            }
        }
        self.metrics.set_observability_tail_sessions(sessions.len());
        Ok(())
    }

    /// Return whether a session is absent or expired, reclaiming its slot.
    pub(crate) fn tail_expired(&self, id: &str) -> bool {
        let now = now_ms().unwrap_or(i64::MAX);
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|_, session| session.expires_at_ms > now);
        self.metrics.set_observability_tail_sessions(sessions.len());
        !sessions.contains_key(id)
    }

    /// Close and revoke one Dashboard Live Tail after its socket ends.
    pub(crate) fn close_live_tail(&self, id: &str) {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions
            .get(id)
            .is_some_and(|session| matches!(&session.protocol, TailProtocol::Live { .. }))
        {
            sessions.remove(id);
        }
        self.metrics.set_observability_tail_sessions(sessions.len());
    }

    /// Release the single-connection claim while retaining the session until DELETE/expiry.
    pub(crate) fn disconnect_tail(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            if sessions
                .get(id)
                .is_some_and(|session| session.drop_streak >= MAX_DROP_STREAK)
            {
                sessions.remove(id);
                self.metrics.set_observability_tail_sessions(sessions.len());
                return;
            }
            let Some(session) = sessions.get_mut(id) else {
                return;
            };
            session.connected = false;
            session.sender = None;
            session.queued_bytes.store(0, Ordering::Relaxed);
            session.overloaded = false;
            session.drop_streak = 0;
        }
    }

    /// Whether a closed receiver was terminated by the bounded slow-client policy.
    pub(crate) fn tail_overloaded(&self, id: &str) -> bool {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| {
                sessions
                    .get(id)
                    .map(|session| session.drop_streak >= MAX_DROP_STREAK)
            })
            .unwrap_or(false)
    }

    pub(super) fn fan_out(
        &self,
        target: &EffectiveIdentity,
        invocation: &NewObservabilityInvocation,
    ) {
        let now = invocation.received_at_ms;
        let mut close = Vec::new();
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        sessions.retain(|_, session| session.expires_at_ms > now);
        for session in sessions.values_mut() {
            if session.account_id != target.account_id || session.worker_id != target.worker.id {
                continue;
            }
            let Some(sender) = session.sender.as_ref() else {
                continue;
            };
            let live = matches!(&session.protocol, TailProtocol::Live { .. });
            let frames = match &session.protocol {
                TailProtocol::Script(filters) => {
                    if matches_tail(filters, invocation, &session.id) {
                        serde_json::to_string(&invocation.event)
                            .ok()
                            .into_iter()
                            .collect::<Vec<_>>()
                    } else {
                        self.metrics.observe_observability_tail_event(false);
                        Vec::new()
                    }
                }
                TailProtocol::Live {
                    combination,
                    filters,
                } => invocation
                    .events
                    .iter()
                    .filter_map(|event| {
                        let event = live_event(invocation, event);
                        match crate::observability_filter::matches(filters, *combination, &event) {
                            Ok(true) => serde_json::to_string(&event).ok(),
                            Ok(false) | Err(_) => {
                                self.metrics.observe_observability_tail_event(false);
                                None
                            }
                        }
                    })
                    .collect(),
            };
            for text in frames {
                let bytes = text.len();
                let queued = session.queued_bytes.fetch_add(bytes, Ordering::Relaxed);
                let limit =
                    usize::try_from(self.config.tail_client_queue_bytes).unwrap_or(usize::MAX);
                if queued.saturating_add(bytes) > limit {
                    session.queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
                    session.drop_streak = session.drop_streak.saturating_add(1);
                    self.metrics.inc_observability_tail_dropped(true);
                    if !session.overloaded {
                        session.overloaded = true;
                        enqueue_overload(sender, &session.queued_bytes, true, live);
                    }
                } else {
                    let frame = TailFrame {
                        text,
                        bytes,
                        queued_bytes: session.queued_bytes.clone(),
                    };
                    if sender.try_send(frame).is_err() {
                        session.drop_streak = session.drop_streak.saturating_add(1);
                        self.metrics.inc_observability_tail_dropped(false);
                    } else {
                        self.metrics.observe_observability_tail_event(true);
                        if session.overloaded && queued < limit / 2 {
                            session.overloaded = false;
                            enqueue_overload(sender, &session.queued_bytes, false, live);
                        }
                        session.drop_streak = 0;
                    }
                }
                if session.drop_streak >= MAX_DROP_STREAK {
                    close.push(session.id.clone());
                    break;
                }
            }
        }
        for id in close {
            if let Some(session) = sessions.get_mut(&id) {
                session.sender = None;
            }
        }
    }

    fn sign_ticket(
        &self,
        id: &str,
        account_id: AccountId,
        worker_id: WorkerId,
        expires_at_ms: i64,
    ) -> Result<String, PlatformError> {
        let mut mac = HmacSha256::new_from_slice(&self.signing_key).map_err(|_| unavailable())?;
        mac.update(ticket_claim(id, account_id, worker_id, expires_at_ms).as_bytes());
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }

    fn verify_ticket(&self, session: &TailSession, ticket: &str) -> bool {
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.signing_key) else {
            return false;
        };
        mac.update(
            ticket_claim(
                &session.id,
                session.account_id,
                session.worker_id,
                session.expires_at_ms,
            )
            .as_bytes(),
        );
        let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(ticket) else {
            return false;
        };
        mac.verify_slice(&bytes).is_ok()
    }

    fn tail_url(&self, id: &str, ticket: &str) -> Result<String, PlatformError> {
        self.websocket_url("tails", id, ticket)
    }

    fn live_tail_url(&self, id: &str, ticket: &str) -> Result<String, PlatformError> {
        self.websocket_url("live-tails", id, ticket)
    }

    fn websocket_url(
        &self,
        resource: &str,
        id: &str,
        ticket: &str,
    ) -> Result<String, PlatformError> {
        let mut url =
            url::Url::parse(&self.config.external_control_origin).map_err(|_| invalid())?;
        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => return Err(invalid()),
        };
        url.set_scheme(scheme).map_err(|()| invalid())?;
        url.set_path(&format!("/client/v4/open-compute/{resource}/{id}/{ticket}"));
        url.set_query(None);
        url.set_fragment(None);
        Ok(url.into())
    }
}
