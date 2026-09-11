//! Typed resource lifecycle and immutable version-binding repository.

use crate::ControlDb;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, RequestId, ResourceAvailability, ResourceId,
    ResourceState,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use std::str::FromStr;

/// Persisted resource authority row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRecord {
    /// Immutable resource identity.
    pub id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Static product kind.
    pub kind: BindingKind,
    /// Account-and-kind-local display name.
    pub name: String,
    /// Durable lifecycle state.
    pub state: ResourceState,
    /// Independent persisted health state.
    pub availability: ResourceAvailability,
    /// Stable health reason when not healthy.
    pub availability_code: Option<String>,
    /// Binding-breaking specification generation.
    pub spec_generation: u64,
    /// Product driver schema version.
    pub driver_schema_version: u32,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last mutation timestamp.
    pub updated_at_ms: i64,
    /// Tombstone timestamp.
    pub deleted_at_ms: Option<i64>,
}

/// Registered reason a resource identity must remain reachable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceReferrer {
    /// Referenced resource.
    pub resource_id: ResourceId,
    /// Owning subsystem token.
    pub referrer_kind: String,
    /// Stable subsystem-local identity.
    pub referrer_id: String,
    /// Registration timestamp.
    pub created_at_ms: i64,
}

/// Create-idempotency reservation outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceCreateReservation {
    /// New creating row was inserted atomically with the idempotency row.
    Reserved(ResourceRecord),
    /// Same running operation must reconcile this existing resource identity.
    Continue(ResourceRecord),
    /// Same operation already completed; value is the exact response bytes.
    Complete(Vec<u8>),
    /// Same operation deterministically failed; value is the persisted envelope.
    Failed(Vec<u8>),
}

/// Delete-idempotency reservation outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceDeleteReservation {
    /// New delete operation owns its durable reservation.
    Reserved(ResourceRecord),
    /// A prior interrupted delete must continue from persisted resource state.
    Continue(ResourceRecord),
    /// The operation already completed; value is the exact response bytes.
    Complete(Vec<u8>),
    /// The operation already failed deterministically; value is the persisted envelope.
    Failed(Vec<u8>),
}

/// Input for an atomic resource-delete idempotency reservation.
#[derive(Clone, Debug)]
pub struct ReserveResourceDelete<'a> {
    /// Owning account.
    pub account_id: AccountId,
    /// Resource selected by the account-scoped route.
    pub resource_id: ResourceId,
    /// Required idempotency key.
    pub idempotency_key: &'a str,
    /// Master-key fingerprint identifier.
    pub fingerprint_key_id: &'a str,
    /// Secret-keyed canonical request fingerprint.
    pub request_fingerprint: &'a [u8; 32],
    /// Reservation timestamp.
    pub now_ms: i64,
    /// Idempotency expiry timestamp.
    pub expires_at_ms: i64,
}

/// Input for atomic resource-create reservation.
#[derive(Clone, Debug)]
pub struct ReserveResourceCreate<'a> {
    /// Owning account.
    pub account_id: AccountId,
    /// Product kind.
    pub kind: BindingKind,
    /// Display name.
    pub name: &'a str,
    /// Required idempotency key.
    pub idempotency_key: &'a str,
    /// Master-key fingerprint used for request HMAC.
    pub fingerprint_key_id: &'a str,
    /// Secret-keyed canonical request fingerprint.
    pub request_fingerprint: &'a [u8; 32],
    /// Identity allocated for a first insertion.
    pub resource_id: ResourceId,
    /// Product driver schema version.
    pub driver_schema_version: u32,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Transaction timestamp.
    pub now_ms: i64,
    /// Idempotency expiry timestamp.
    pub expires_at_ms: i64,
}

/// Resource and binding authority over `control.sqlite`.
#[derive(Clone, Copy, Debug)]
pub struct ResourceRepository<'a> {
    db: &'a ControlDb,
}

type ExistingCreate = (Vec<u8>, String, Option<Vec<u8>>, Option<String>);

mod repository;

pub(crate) fn read_resource_conn(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<ResourceRecord, PlatformError> {
    conn.query_row(
        "SELECT id, account_id, kind, name, state, availability,
                availability_code, spec_generation, driver_schema_version,
                created_at_ms, updated_at_ms, deleted_at_ms
         FROM resources WHERE id = ?1 AND account_id = ?2",
        params![resource_id.to_string(), account_id.to_string()],
        map_resource,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(resource_not_found)
}

fn read_resource_tx(
    tx: &Transaction<'_>,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<ResourceRecord, PlatformError> {
    read_resource_conn(tx, account_id, resource_id)
}

fn has_referrers(tx: &Transaction<'_>, resource_id: ResourceId) -> Result<bool, PlatformError> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM resource_referrers WHERE resource_id = ?1)
             OR EXISTS(SELECT 1 FROM ai_search_r2_sources WHERE bucket_resource_id = ?1)",
        [resource_id.to_string()],
        |row| row.get(0),
    )
    .map_err(|_| db_error())
}

fn map_ai_search_r2_referrer(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceReferrer> {
    let resource: String = row.get(0)?;
    Ok(ResourceReferrer {
        resource_id: ResourceId::from_str(&resource).map_err(|_| rusqlite::Error::InvalidQuery)?,
        referrer_kind: "ai_search_r2_source".to_owned(),
        referrer_id: row.get(1)?,
        created_at_ms: row.get(2)?,
    })
}

fn map_resource(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceRecord> {
    map_resource_offset(row, 0)
}

pub(crate) fn map_resource_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ResourceRecord> {
    let id: String = row.get(offset)?;
    let account: String = row.get(offset + 1)?;
    let kind: String = row.get(offset + 2)?;
    let state: String = row.get(offset + 4)?;
    let availability: String = row.get(offset + 5)?;
    let generation: i64 = row.get(offset + 7)?;
    let schema: i64 = row.get(offset + 8)?;
    Ok(ResourceRecord {
        id: ResourceId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: BindingKind::from_str(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(offset + 3)?,
        state: ResourceState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: ResourceAvailability::from_str(&availability)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability_code: row.get(offset + 6)?,
        spec_generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        driver_schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(offset + 9)?,
        updated_at_ms: row.get(offset + 10)?,
        deleted_at_ms: row.get(offset + 11)?,
    })
}

fn map_referrer(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceReferrer> {
    let resource: String = row.get(0)?;
    Ok(ResourceReferrer {
        resource_id: ResourceId::from_str(&resource).map_err(|_| rusqlite::Error::InvalidQuery)?,
        referrer_kind: row.get(1)?,
        referrer_id: row.get(2)?,
        created_at_ms: row.get(3)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(|_| resource_invariant())?);
    }
    Ok(output)
}

fn require_account(tx: &Transaction<'_>, account_id: AccountId) -> Result<(), PlatformError> {
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1 AND deleted_at_ms IS NULL)",
            [account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if !exists {
        return Err(resource_not_found());
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), PlatformError> {
    if name.is_empty()
        || name.chars().count() > 128
        || name.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(resource_invariant());
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> Result<(), PlatformError> {
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "idempotency key is invalid",
        ));
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "SQLite boundary inputs mirror authoritative persisted fields"
)]
fn audit(
    tx: &Transaction<'_>,
    account_id: AccountId,
    action: &str,
    target_type: &str,
    target_id: &str,
    request_id: RequestId,
    details: &[u8],
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO control_audit_events
         (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            account_id.to_string(),
            action,
            target_type,
            target_id,
            request_id.to_string(),
            details,
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

fn resource_not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "resource was not found")
}

fn resource_not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotReady,
        "resource lifecycle does not admit this operation",
    )
}

fn resource_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "persisted resource invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
