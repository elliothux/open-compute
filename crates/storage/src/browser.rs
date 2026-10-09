//! Durable instance-owned Browser Run session metadata; browser state is ephemeral.

use crate::control_db::ControlDb;
use crate::worker_repository::require_instance;
use open_compute_core::{ErrorCode, InstanceId, PlatformError};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Nonsecret browser session state stored in the control authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSessionState {
    /// Ready for an authenticated reconnect.
    Ready,
    /// One or more authenticated CDP connections are attached.
    Connected,
    /// Context disposal has begun; no new commands are admitted.
    Closing,
    /// Explicit close or expiry completed.
    Closed,
    /// Its process/transport generation was lost.
    Lost,
}

impl BrowserSessionState {
    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "ready" => Ok(Self::Ready),
            "connected" => Ok(Self::Connected),
            "closing" => Ok(Self::Closing),
            "closed" => Ok(Self::Closed),
            "lost" => Ok(Self::Lost),
            _ => Err(invariant()),
        }
    }
}

/// Durable terminal cause, independent of the session lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserSessionCloseReason {
    /// Client or action explicitly ended the session.
    Normal,
    /// Its configured command inactivity deadline elapsed.
    Idle,
    /// The owning process or transport generation disappeared.
    Lost,
}
impl BrowserSessionCloseReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Idle => "idle",
            Self::Lost => "lost",
        }
    }
    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "normal" => Ok(Self::Normal),
            "idle" => Ok(Self::Idle),
            "lost" => Ok(Self::Lost),
            _ => Err(invariant()),
        }
    }
}

/// Public identity and audit metadata, without an engine/context/endpoint locator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserSessionRecord {
    /// Opaque platform session identifier.
    pub id: String,
    /// Immutable instance ownership.
    pub instance_id: InstanceId,
    /// Ephemeral process/transport generation.
    pub generation: String,
    /// Digest of its frozen runtime contract.
    pub contract_sha256: [u8; 32],
    /// Current lifecycle state.
    pub state: BrowserSessionState,
    /// Qualified idle lifetime, in milliseconds.
    pub keep_alive_ms: u64,
    /// Number of active CDP connections.
    pub connections: u32,
    /// Audit creation timestamp.
    pub created_at_ms: i64,
    /// Most recent real command/action activity timestamp.
    pub last_activity_at_ms: i64,
    /// Most recent connection establishment timestamp.
    pub connected_at_ms: Option<i64>,
    /// Terminal timestamp, if closed or lost.
    pub closed_at_ms: Option<i64>,
    /// Terminal cause; absent while admission remains live.
    pub close_reason: Option<BrowserSessionCloseReason>,
}

/// Synchronous SQLite session authority for one instance database.
#[derive(Clone, Copy, Debug)]
pub struct BrowserSessions<'a> {
    db: &'a ControlDb,
}

impl<'a> BrowserSessions<'a> {
    /// Bind the control authority; no process work happens inside transactions.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Publish a ready session only after its context/backend allocation succeeds.
    pub fn create(
        &self,
        record: &BrowserSessionRecord,
        max_sessions: u32,
    ) -> Result<(), PlatformError> {
        if uuid::Uuid::parse_str(&record.id).is_err()
            || uuid::Uuid::parse_str(&record.generation).is_err()
            || record.state != BrowserSessionState::Ready
            || record.connections != 0
            || record.closed_at_ms.is_some()
            || record.close_reason.is_some()
            || record.connected_at_ms.is_some()
            || !(10_000..=1_200_000).contains(&record.keep_alive_ms)
            || max_sessions == 0
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, record.instance_id)?;
            let count: u32 = tx.query_row(
                "SELECT count(*) FROM browser_sessions WHERE state IN ('ready','connected','closing')",
                [], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if count >= max_sessions {
                return Err(PlatformError::new(ErrorCode::AdmissionBusy, "browser session capacity exhausted"));
            }
            tx.execute(
                "INSERT INTO browser_sessions
                 (id, instance_id, runtime_generation, runtime_contract_sha256, state, keep_alive_ms,
                  created_at_ms, last_activity_at_ms) VALUES (?1,?2,?3,?4,'ready',?5,?6,?7)",
                params![record.id, record.instance_id.to_string(), record.generation,
                    record.contract_sha256.as_slice(), record.keep_alive_ms, record.created_at_ms, record.last_activity_at_ms],
            ).map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Read only within the supplied instance and exact live generation fence.
    pub fn get(
        &self,
        instance: InstanceId,
        id: &str,
        generation: &str,
    ) -> Result<Option<BrowserSessionRecord>, PlatformError> {
        self.db.with_read(|connection| {
            connection.query_row(
                "SELECT id, instance_id, runtime_generation, runtime_contract_sha256, state, keep_alive_ms,
                 connections, created_at_ms, last_activity_at_ms, connected_at_ms, closed_at_ms, close_reason
                 FROM browser_sessions WHERE instance_id=?1 AND id=?2 AND runtime_generation=?3",
                params![instance.to_string(), id, generation], decode,
            ).optional().map_err(|_| invariant())?.map(validate).transpose()
        })
    }

    /// Read a bounded current list or retained session history in stable creation order.
    pub fn list(
        &self,
        instance: InstanceId,
        history: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<BrowserSessionRecord>, PlatformError> {
        if limit == 0 || limit > 1024 || offset > 1_000_000 {
            return Err(invariant());
        }
        self.db.with_read(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, instance_id, runtime_generation, runtime_contract_sha256, state, keep_alive_ms,
                 connections, created_at_ms, last_activity_at_ms, connected_at_ms, closed_at_ms, close_reason
                 FROM browser_sessions WHERE instance_id=?1
                 AND ((?2 AND state IN ('closed','lost'))
                   OR (NOT ?2 AND state NOT IN ('closed','lost')))
                 ORDER BY created_at_ms,id LIMIT ?3 OFFSET ?4",
            ).map_err(|_| invariant())?;
            statement.query_map(params![instance.to_string(), history, limit, offset], decode)
                .map_err(|_| invariant())?.map(|row| validate(row.map_err(|_| invariant())?)).collect()
        })
    }

    /// Admit or release one connection; listing/pings never count as activity.
    pub fn connection(
        &self,
        instance: InstanceId,
        id: &str,
        generation: &str,
        connected: bool,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let delta: i32 = if connected { 1 } else { -1 };
            let changed = tx
                .execute(
                    "UPDATE browser_sessions SET connections=connections+?4,
                 state=CASE WHEN connections+?4=0 THEN 'ready' ELSE 'connected' END,
                 connected_at_ms=CASE WHEN ?4=1 THEN ?5 ELSE connected_at_ms END
                 WHERE instance_id=?1 AND id=?2 AND runtime_generation=?3
                   AND state IN ('ready','connected') AND connections+?4>=0",
                    params![instance.to_string(), id, generation, delta, now_ms],
                )
                .map_err(|_| invariant())?;
            if changed != 1 {
                return Err(not_found());
            }
            Ok(())
        })
    }

    /// Record real command/action activity under the immutable generation fence.
    pub fn activity(
        &self,
        instance: InstanceId,
        id: &str,
        generation: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx.execute(
                "UPDATE browser_sessions SET last_activity_at_ms=?4
                 WHERE instance_id=?1 AND id=?2 AND runtime_generation=?3 AND state IN ('ready','connected')",
                params![instance.to_string(), id, generation, now_ms],
            ).map_err(|_| invariant())?;
            if changed != 1 { return Err(not_found()); }
            Ok(())
        })
    }

    /// Fence further admission before beginning context disposal.
    pub fn begin_close(
        &self,
        instance: InstanceId,
        id: &str,
        generation: &str,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE browser_sessions SET state='closing' WHERE instance_id=?1 AND id=?2
             AND runtime_generation=?3 AND state IN ('ready','connected')",
                params![instance.to_string(), id, generation],
            )
            .map(|changed| changed == 1)
            .map_err(|_| invariant())
        })
    }

    /// Commit verified disposal, or fence an irrecoverably lost session, idempotently.
    pub fn finish_close(
        &self,
        instance: InstanceId,
        id: &str,
        generation: &str,
        reason: BrowserSessionCloseReason,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE browser_sessions SET state=?4, connections=0, closed_at_ms=?5, close_reason=?6
                 WHERE instance_id=?1 AND id=?2 AND runtime_generation=?3
                   AND state NOT IN ('closed','lost') AND (?4='lost' OR state='closing')",
                params![
                    instance.to_string(),
                    id,
                    generation,
                    if reason == BrowserSessionCloseReason::Lost { "lost" } else { "closed" },
                    now_ms,
                    reason.as_str()
                ],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Daemon restart invalidates every old ephemeral transport generation.
    pub fn lose_all(&self, instance: InstanceId, now_ms: i64) -> Result<usize, PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE browser_sessions SET state='lost', connections=0, closed_at_ms=?2, close_reason='lost'
             WHERE instance_id=?1 AND state NOT IN ('closed','lost')",
                params![instance.to_string(), now_ms],
            )
            .map_err(|_| invariant())
        })
    }

    /// Delete only expired or excess terminal metadata, retaining the most recently closed records.
    /// Live and closing sessions are independent of this budget; equal close times use ID order.
    pub fn retain_history(
        &self,
        instance: InstanceId,
        cutoff_ms: i64,
        max_entries: u32,
    ) -> Result<usize, PlatformError> {
        if !(1..=100_000).contains(&max_entries) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            tx.execute(
                "DELETE FROM browser_sessions
                 WHERE instance_id=?1 AND state IN ('closed','lost')
                   AND (closed_at_ms<=?2 OR id NOT IN (
                     SELECT id FROM browser_sessions
                     WHERE instance_id=?1 AND state IN ('closed','lost')
                     ORDER BY closed_at_ms DESC,id DESC LIMIT ?3
                   ))",
                params![instance.to_string(), cutoff_ms, max_entries],
            )
            .map_err(|_| invariant())
        })
    }
}

type Stored = (
    String,
    String,
    String,
    Vec<u8>,
    String,
    u64,
    u32,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}
fn validate(row: Stored) -> Result<BrowserSessionRecord, PlatformError> {
    Ok(BrowserSessionRecord {
        id: row.0,
        instance_id: row.1.parse().map_err(|_| invariant())?,
        generation: row.2,
        contract_sha256: row.3.try_into().map_err(|_| invariant())?,
        state: BrowserSessionState::parse(&row.4)?,
        keep_alive_ms: row.5,
        connections: row.6,
        created_at_ms: row.7,
        last_activity_at_ms: row.8,
        connected_at_ms: row.9,
        closed_at_ms: row.10,
        close_reason: row
            .11
            .as_deref()
            .map(BrowserSessionCloseReason::parse)
            .transpose()?,
    })
}
fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "browser session authority invariant failed",
    )
}
fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "browser session not found")
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
