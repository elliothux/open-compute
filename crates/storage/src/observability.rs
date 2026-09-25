//! Independent bounded SQLite authority for Workers Logs.

use open_compute_core::{ErrorCode, InstanceId, PlatformError};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;

mod schema;
use schema::{bind_instance, migrate, quick_check};
mod types;

pub use types::{
    ObservabilityFieldKey, ObservabilityFieldValue, ObservabilityUsage, ObservabilityUsageBreakdown,
};

const DATA_FORMAT: &str = "open-compute-observability-v1";
const QUERY_READ_MAX_BYTES: usize = 32 * 1024 * 1024;

/// One bounded scalar indexed for telemetry field discovery and filtering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityField {
    /// Canonical dotted field key.
    pub key: String,
    /// Scalar value.
    pub value: Value,
}

/// One projected Workers Logs event ready for insertion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewObservabilityEvent {
    /// Globally opaque event identity.
    pub event_id: String,
    /// Invocation-local sequence.
    pub sequence: u32,
    /// Event timestamp in Unix milliseconds.
    pub timestamp_ms: i64,
    /// `cf-worker-event` or `cf-worker-log`.
    pub metadata_type: String,
    /// Optional console level.
    pub level: Option<String>,
    /// Public structured or string source value.
    pub source: Value,
    /// Complete public telemetry metadata.
    pub metadata: Value,
    /// Bounded indexed scalar fields.
    pub fields: Vec<ObservabilityField>,
}

/// One canonical invocation and its public event projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewObservabilityInvocation {
    /// Opaque invocation identity.
    pub invocation_id: String,
    /// Owning instance identity.
    pub instance_id: InstanceId,
    /// External Script name.
    pub script_name: String,
    /// External immutable Version identity.
    pub version_id: String,
    /// Deployment identity, when the execution was deployment-backed.
    pub deployment_id: Option<String>,
    /// Invocation event timestamp in Unix milliseconds.
    pub event_timestamp_ms: i64,
    /// Platform receive timestamp in Unix milliseconds.
    pub received_at_ms: i64,
    /// Stable event-type token.
    pub event_type: String,
    /// Workerd outcome token.
    pub outcome: String,
    /// CPU milliseconds reported by workerd.
    pub cpu_time_ms: f64,
    /// Wall milliseconds reported by workerd.
    pub wall_time_ms: f64,
    /// Whether any canonical projection was truncated.
    pub truncated: bool,
    /// Canonical redacted trace-v1 representation.
    pub event: Value,
    /// Persisted public events.
    pub events: Vec<NewObservabilityEvent>,
}

/// Stable event pagination boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservabilityEventCursor {
    /// Last timestamp already returned.
    pub timestamp_ms: i64,
    /// Last event identity already returned.
    pub event_id: String,
}

/// One stored Workers Logs event.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredObservabilityEvent {
    /// Opaque event identity.
    pub event_id: String,
    /// Parent invocation identity.
    pub invocation_id: String,
    /// External Script name.
    pub script_name: String,
    /// External Version identity.
    pub version_id: String,
    /// Event timestamp.
    pub timestamp_ms: i64,
    /// Invocation-local sequence.
    pub sequence: u32,
    /// Cloudflare metadata type.
    pub metadata_type: String,
    /// Optional console level.
    pub level: Option<String>,
    /// Public event source.
    pub source: Value,
    /// Public metadata object.
    pub metadata: Value,
}

/// Single-process owner of `observability.sqlite`.
#[derive(Debug)]
pub struct ObservabilityStore {
    connection: Mutex<Connection>,
    instance_id: InstanceId,
    retention_ms: i64,
    max_database_bytes: u64,
}

impl ObservabilityStore {
    /// Open, initialize, and integrity-check the current Day 1 schema.
    pub fn open(
        path: &Path,
        instance_id: InstanceId,
        busy_timeout_ms: u64,
        retention_ms: u64,
        max_database_bytes: u64,
    ) -> Result<Self, PlatformError> {
        let retention_ms = i64::try_from(retention_ms).map_err(|_| invalid())?;
        let path = crate::control_db::leaf_nofollow_path(path)?;
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| unavailable())?;
        connection
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .map_err(|_| unavailable())?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .and_then(|()| connection.pragma_update(None, "journal_mode", "WAL"))
            .and_then(|()| connection.pragma_update(None, "synchronous", "FULL"))
            .map_err(|_| unavailable())?;
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(|_| unavailable())?;
        let max_pages = max_database_bytes
            .checked_div(page_size)
            .ok_or_else(invalid)?;
        if max_pages == 0 {
            return Err(invalid());
        }
        connection
            .pragma_update(None, "max_page_count", max_pages)
            .map_err(|_| unavailable())?;
        migrate(&mut connection)?;
        quick_check(&connection)?;
        bind_instance(&connection, instance_id)?;
        Ok(Self {
            connection: Mutex::new(connection),
            instance_id,
            retention_ms,
            max_database_bytes,
        })
    }

    /// Insert one idempotent invocation and its events.
    pub fn insert(&self, invocation: &NewObservabilityInvocation) -> Result<bool, PlatformError> {
        Ok(self.insert_batch(std::slice::from_ref(invocation))? == 1)
    }

    /// Insert a bounded batch in one immediate transaction.
    pub fn insert_batch(
        &self,
        invocations: &[NewObservabilityInvocation],
    ) -> Result<usize, PlatformError> {
        if invocations.is_empty() || invocations.len() > 4_096 {
            return Err(invalid());
        }
        for invocation in invocations {
            if invocation.instance_id != self.instance_id {
                return Err(unavailable());
            }
            validate_invocation(invocation)?;
        }
        let mut connection = self.connection.lock().map_err(|_| unavailable())?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        let newest_received_at_ms = invocations
            .iter()
            .map(|value| value.received_at_ms)
            .max()
            .ok_or_else(invalid)?;
        let cutoff = newest_received_at_ms.saturating_sub(self.retention_ms);
        tx.execute(
            "DELETE FROM observability_invocations WHERE received_at_ms < ?1",
            [cutoff],
        )
        .map_err(|_| unavailable())?;
        let accounted: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(byte_size), 0) FROM observability_invocations",
                [],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        let mut accounted = u64::try_from(accounted).map_err(|_| unavailable())?;
        let mut inserted_count = 0_usize;
        for invocation in invocations {
            if insert_invocation(&tx, invocation, &mut accounted, self.max_database_bytes)? {
                inserted_count += 1;
            }
        }
        tx.execute(
            "UPDATE observability_maintenance SET accounted_bytes=?1, last_gc_at_ms=?2
             WHERE singleton=1",
            params![
                i64::try_from(accounted).map_err(|_| invalid())?,
                newest_received_at_ms
            ],
        )
        .map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(inserted_count)
    }

    /// Read a bounded descending page of public events.
    pub fn query_events(
        &self,
        instance_id: InstanceId,
        from_ms: i64,
        to_ms: i64,
        script_name: Option<&str>,
        cursor: Option<&ObservabilityEventCursor>,
        limit: u32,
    ) -> Result<Vec<StoredObservabilityEvent>, PlatformError> {
        if instance_id != self.instance_id {
            return Err(unavailable());
        }
        if from_ms >= to_ms || limit == 0 || limit > 20_000 {
            return Err(invalid());
        }
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        if let Some(cursor) = cursor {
            let anchor = connection
                .query_row(
                    "SELECT 1 FROM observability_events
                     WHERE timestamp_ms=?1 AND event_id=?2",
                    params![cursor.timestamp_ms, cursor.event_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|_| unavailable())?;
            if anchor.is_none() {
                return Err(cursor_expired());
            }
        }
        let mut output = Vec::new();
        let mut statement = connection
            .prepare(
                "SELECT event_id, invocation_id, script_name, version_id, timestamp_ms, sequence,
                        metadata_type, level, source_json, metadata_json
                 FROM observability_events
                 WHERE timestamp_ms>=?1 AND timestamp_ms<?2
                   AND (?3 IS NULL OR script_name=?3)
                   AND (?4 IS NULL OR timestamp_ms<?4 OR (timestamp_ms=?4 AND event_id<?5))
                 ORDER BY timestamp_ms DESC, event_id DESC LIMIT ?6",
            )
            .map_err(|_| unavailable())?;
        let rows = statement
            .query_map(
                params![
                    from_ms,
                    to_ms,
                    script_name,
                    cursor.map(|value| value.timestamp_ms),
                    cursor.map(|value| value.event_id.as_str()),
                    limit,
                ],
                map_event,
            )
            .map_err(|_| unavailable())?;
        let mut bytes = 0_usize;
        for row in rows {
            let event = row.map_err(|_| unavailable())?;
            bytes = bytes
                .saturating_add(
                    serde_json::to_vec(&event.source)
                        .map_err(|_| unavailable())?
                        .len(),
                )
                .saturating_add(
                    serde_json::to_vec(&event.metadata)
                        .map_err(|_| unavailable())?
                        .len(),
                );
            if bytes > QUERY_READ_MAX_BYTES {
                return Err(query_limit());
            }
            output.push(event);
        }
        Ok(output)
    }

    /// Discover bounded indexed keys in a retention window.
    pub fn keys(
        &self,
        instance_id: InstanceId,
        from_ms: i64,
        to_ms: i64,
        limit: u32,
    ) -> Result<Vec<ObservabilityFieldKey>, PlatformError> {
        if instance_id != self.instance_id {
            return Err(unavailable());
        }
        if from_ms >= to_ms || limit == 0 || limit > 10_000 {
            return Err(invalid());
        }
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT f.key, f.value_type, MAX(e.timestamp_ms)
                 FROM observability_fields f JOIN observability_events e ON e.event_id=f.event_id
                 WHERE e.timestamp_ms>=?1 AND e.timestamp_ms<?2
                 GROUP BY f.key, f.value_type ORDER BY f.key, f.value_type LIMIT ?3",
            )
            .map_err(|_| unavailable())?;
        let rows = statement
            .query_map(params![from_ms, to_ms, limit], |row| {
                Ok(ObservabilityFieldKey {
                    key: row.get(0)?,
                    value_type: row.get(1)?,
                    last_seen_at: row.get(2)?,
                })
            })
            .map_err(|_| unavailable())?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row.map_err(|_| unavailable())?);
        }
        Ok(output)
    }

    /// Read bounded distinct scalar values for one indexed key.
    pub fn values(
        &self,
        instance_id: InstanceId,
        key: &str,
        value_type: &str,
        from_ms: i64,
        to_ms: i64,
        limit: u32,
    ) -> Result<Vec<ObservabilityFieldValue>, PlatformError> {
        if instance_id != self.instance_id {
            return Err(unavailable());
        }
        if key.is_empty()
            || !matches!(value_type, "string" | "number" | "boolean")
            || from_ms >= to_ms
            || limit == 0
            || limit > 2_000
        {
            return Err(invalid());
        }
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT f.value_type, f.string_value, f.number_value, f.boolean_value
                 FROM observability_fields f JOIN observability_events e ON e.event_id=f.event_id
                 WHERE f.key=?1 AND f.value_type=?2
                   AND e.timestamp_ms>=?3 AND e.timestamp_ms<?4
                 ORDER BY f.string_value, f.number_value, f.boolean_value LIMIT ?5",
            )
            .map_err(|_| unavailable())?;
        let rows = statement
            .query_map(params![key, value_type, from_ms, to_ms, limit], |row| {
                let value_type: String = row.get(0)?;
                let value = match value_type.as_str() {
                    "string" => Value::String(row.get(1)?),
                    "number" => serde_json::Number::from_f64(row.get(2)?)
                        .map(Value::Number)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    "boolean" => Value::Bool(row.get(3)?),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(ObservabilityFieldValue { value_type, value })
            })
            .map_err(|_| unavailable())?;
        let mut output = Vec::new();
        for row in rows {
            output.push(row.map_err(|_| unavailable())?);
        }
        Ok(output)
    }

    /// Aggregate Cloudflare-compatible Workers Logs usage for one account.
    pub fn usage(
        &self,
        instance_id: InstanceId,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<ObservabilityUsage, PlatformError> {
        if instance_id != self.instance_id {
            return Err(unavailable());
        }
        if from_ms >= to_ms {
            return Err(invalid());
        }
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT strftime('%Y-%m-%d 00:00:00', timestamp_ms / 1000, 'unixepoch'),
                        script_name, COUNT(*)
                 FROM observability_events
                 WHERE timestamp_ms>=?1 AND timestamp_ms<?2
                 GROUP BY 1, script_name ORDER BY 1, script_name",
            )
            .map_err(|_| unavailable())?;
        let rows = statement
            .query_map(params![from_ms, to_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| unavailable())?;
        let mut events = 0_u64;
        let mut breakdown = Vec::new();
        for row in rows {
            let (bin, service, count) = row.map_err(|_| unavailable())?;
            let count = u64::try_from(count).map_err(|_| unavailable())?;
            events = events.checked_add(count).ok_or_else(unavailable)?;
            breakdown.push(ObservabilityUsageBreakdown {
                bin,
                dataset: "cloudflare-workers",
                service,
                count,
            });
        }
        Ok(ObservabilityUsage { events, breakdown })
    }

    /// Delete expired rows in one bounded maintenance transaction.
    pub fn prune(&self, now_ms: i64, maximum: u32) -> Result<u64, PlatformError> {
        if maximum == 0 || maximum > 100_000 {
            return Err(invalid());
        }
        let mut connection = self.connection.lock().map_err(|_| unavailable())?;
        let cutoff = now_ms.saturating_sub(self.retention_ms);
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| unavailable())?;
        let removed = tx
            .execute(
                "DELETE FROM observability_invocations WHERE invocation_id IN
                 (SELECT invocation_id FROM observability_invocations WHERE received_at_ms<?1
                  ORDER BY received_at_ms LIMIT ?2)",
                params![cutoff, maximum],
            )
            .map_err(|_| unavailable())?;
        let accounted: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(byte_size), 0) FROM observability_invocations",
                [],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        tx.execute(
            "UPDATE observability_maintenance SET accounted_bytes=?1, last_gc_at_ms=?2
             WHERE singleton=1",
            params![accounted, now_ms],
        )
        .map_err(|_| unavailable())?;
        tx.commit().map_err(|_| unavailable())?;
        u64::try_from(removed).map_err(|_| unavailable())
    }

    /// Return the oldest committed event timestamp for status reporting.
    pub fn oldest_event_ms(&self) -> Result<Option<i64>, PlatformError> {
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        connection
            .query_row(
                "SELECT MIN(timestamp_ms) FROM observability_events",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| unavailable())
            .map(Option::flatten)
    }

    /// Return the logical byte accounting used by the hard quota.
    pub fn accounted_bytes(&self) -> Result<u64, PlatformError> {
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let value: i64 = connection
            .query_row(
                "SELECT accounted_bytes FROM observability_maintenance WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| unavailable())?;
        u64::try_from(value).map_err(|_| unavailable())
    }
}

fn validate_invocation(value: &NewObservabilityInvocation) -> Result<(), PlatformError> {
    if value.invocation_id.is_empty()
        || value.invocation_id.len() > 128
        || value.script_name.is_empty()
        || value.script_name.len() > 63
        || value.version_id.is_empty()
        || value.event_type.is_empty()
        || value.outcome.is_empty()
        || !value.cpu_time_ms.is_finite()
        || value.cpu_time_ms < 0.0
        || !value.wall_time_ms.is_finite()
        || value.wall_time_ms < 0.0
        || value.events.len() > 2_048
    {
        return Err(invalid());
    }
    for (index, event) in value.events.iter().enumerate() {
        if event.event_id.is_empty()
            || event.event_id.len() > 160
            || event.sequence as usize != index
            || !matches!(
                event.metadata_type.as_str(),
                "cf-worker-event" | "cf-worker-log"
            )
            || event.fields.len() > 256
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn insert_invocation(
    tx: &Transaction<'_>,
    invocation: &NewObservabilityInvocation,
    accounted: &mut u64,
    max_database_bytes: u64,
) -> Result<bool, PlatformError> {
    let event_json = serde_json::to_vec(&invocation.event).map_err(|_| invalid())?;
    let mut encoded_events = Vec::with_capacity(invocation.events.len());
    let mut logical_bytes = u64::try_from(event_json.len()).map_err(|_| invalid())?;
    for event in &invocation.events {
        let source = serde_json::to_vec(&event.source).map_err(|_| invalid())?;
        let metadata = serde_json::to_vec(&event.metadata).map_err(|_| invalid())?;
        logical_bytes = logical_bytes
            .saturating_add(u64::try_from(source.len()).map_err(|_| invalid())?)
            .saturating_add(u64::try_from(metadata.len()).map_err(|_| invalid())?);
        encoded_events.push((event, source, metadata));
    }
    if logical_bytes == 0 || logical_bytes > max_database_bytes {
        return Err(quota());
    }
    let exists = tx
        .query_row(
            "SELECT 1 FROM observability_invocations WHERE invocation_id=?1",
            [&invocation.invocation_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| unavailable())?
        .is_some();
    if exists {
        return Ok(false);
    }
    while accounted.saturating_add(logical_bytes) > max_database_bytes {
        let oldest = tx
            .query_row(
                "SELECT invocation_id, byte_size FROM observability_invocations
                 ORDER BY received_at_ms, invocation_id LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|_| unavailable())?;
        let Some((invocation_id, byte_size)) = oldest else {
            return Err(quota());
        };
        tx.execute(
            "DELETE FROM observability_invocations WHERE invocation_id=?1",
            [invocation_id],
        )
        .map_err(|_| unavailable())?;
        *accounted = accounted.saturating_sub(u64::try_from(byte_size).map_err(|_| unavailable())?);
    }
    let inserted = tx
        .execute(
            "INSERT OR IGNORE INTO observability_invocations
             (invocation_id, script_name, version_id, deployment_id,
              event_timestamp_ms, received_at_ms, event_type, outcome, cpu_time_ms,
              wall_time_ms, truncated, event_json, byte_size)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                invocation.invocation_id,
                invocation.script_name,
                invocation.version_id,
                invocation.deployment_id,
                invocation.event_timestamp_ms,
                invocation.received_at_ms,
                invocation.event_type,
                invocation.outcome,
                invocation.cpu_time_ms,
                invocation.wall_time_ms,
                invocation.truncated,
                event_json,
                i64::try_from(logical_bytes).map_err(|_| invalid())?,
            ],
        )
        .map_err(|_| unavailable())?;
    if inserted == 0 {
        return Ok(false);
    }
    for (event, source, metadata) in encoded_events {
        let byte_size = source.len().saturating_add(metadata.len());
        tx.execute(
            "INSERT INTO observability_events
             (event_id, invocation_id, script_name, version_id, timestamp_ms,
              sequence, metadata_type, level, source_json, metadata_json, byte_size)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                event.event_id,
                invocation.invocation_id,
                invocation.script_name,
                invocation.version_id,
                event.timestamp_ms,
                event.sequence,
                event.metadata_type,
                event.level,
                source,
                metadata,
                i64::try_from(byte_size).map_err(|_| invalid())?,
            ],
        )
        .map_err(|_| unavailable())?;
        for field in &event.fields {
            insert_field(tx, &event.event_id, field)?;
        }
    }
    *accounted = accounted.saturating_add(logical_bytes);
    Ok(true)
}

fn insert_field(
    tx: &Transaction<'_>,
    event_id: &str,
    field: &ObservabilityField,
) -> Result<(), PlatformError> {
    if field.key.is_empty() || field.key.len() > 512 {
        return Err(invalid());
    }
    let (value_type, string_value, number_value, boolean_value) = match &field.value {
        Value::String(value) if value.len() <= 16_384 => {
            ("string", Some(value.as_str()), None, None)
        }
        Value::Number(value) => {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(invalid)?;
            ("number", None, Some(number), None)
        }
        Value::Bool(value) => ("boolean", None, None, Some(*value)),
        _ => return Err(invalid()),
    };
    tx.execute(
        "INSERT INTO observability_fields
         (event_id,key,value_type,string_value,number_value,boolean_value)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            event_id,
            field.key,
            value_type,
            string_value,
            number_value,
            boolean_value
        ],
    )
    .map_err(|_| unavailable())?;
    Ok(())
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredObservabilityEvent> {
    let sequence: i64 = row.get(5)?;
    Ok(StoredObservabilityEvent {
        event_id: row.get(0)?,
        invocation_id: row.get(1)?,
        script_name: row.get(2)?,
        version_id: row.get(3)?,
        timestamp_ms: row.get(4)?,
        sequence: u32::try_from(sequence).map_err(|_| rusqlite::Error::InvalidQuery)?,
        metadata_type: row.get(6)?,
        level: row.get(7)?,
        source: json_column(row, 8)?,
        metadata: json_column(row, 9)?,
    })
}

fn json_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Value> {
    let bytes: Vec<u8> = row.get(index)?;
    serde_json::from_slice(&bytes).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn invalid() -> PlatformError {
    PlatformError::new(ErrorCode::LimitInvalid, "observability input is invalid")
}

fn quota() -> PlatformError {
    PlatformError::new(
        ErrorCode::QuotaExceeded,
        "observability database quota was exceeded",
    )
}

fn query_limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::AdmissionBusy,
        "observability query exceeded the bounded read budget",
    )
}

fn cursor_expired() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "observability query cursor has expired",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "observability database is unavailable",
    )
}
