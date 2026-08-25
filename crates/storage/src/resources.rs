//! Typed resource lifecycle and immutable deployment-binding repository.

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

impl<'a> ResourceRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Atomically reserve idempotency and insert or recover one creating row.
    pub fn reserve_create(
        &self,
        input: &ReserveResourceCreate<'_>,
    ) -> Result<ResourceCreateReservation, PlatformError> {
        validate_name(input.name)?;
        validate_idempotency_key(input.idempotency_key)?;
        if input.driver_schema_version == 0 || input.expires_at_ms <= input.now_ms {
            return Err(resource_invariant());
        }
        self.db.with_immediate(|tx| {
            require_account(tx, input.account_id)?;
            let existing: Option<ExistingCreate> = tx
                .query_row(
                    "SELECT request_fingerprint, state, response_json, resource_id
                     FROM control_idempotency
                     WHERE account_id = ?1 AND scope = 'resource.create'
                       AND idempotency_key = ?2",
                    params![input.account_id.to_string(), input.idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((fingerprint, state, response, resource)) = existing {
                if fingerprint.as_slice() != input.request_fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "idempotency key fingerprint does not match",
                    ));
                }
                return match state.as_str() {
                    "complete" => response
                        .map(ResourceCreateReservation::Complete)
                        .ok_or_else(resource_invariant),
                    "failed" => response
                        .map(ResourceCreateReservation::Failed)
                        .ok_or_else(resource_invariant),
                    "running" => {
                        let id = resource
                            .ok_or_else(resource_invariant)?
                            .parse::<ResourceId>()
                            .map_err(|_| resource_invariant())?;
                        read_resource_tx(tx, input.account_id, id)
                            .map(ResourceCreateReservation::Continue)
                    }
                    _ => Err(resource_invariant()),
                };
            }

            let name_conflict: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM resources
                     WHERE account_id = ?1 AND kind = ?2 AND name = ?3
                       AND state != 'tombstoned')",
                    params![
                        input.account_id.to_string(),
                        input.kind.as_str(),
                        input.name
                    ],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if name_conflict {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNameConflict,
                    "a live resource already owns this name",
                ));
            }
            tx.execute(
                "INSERT INTO control_idempotency
                 (account_id, scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, deployment_id, state,
                  created_at_ms, expires_at_ms, resource_id)
                 VALUES (?1, 'resource.create', ?2, ?3, ?4, NULL, NULL,
                         'running', ?5, ?6, ?7)",
                params![
                    input.account_id.to_string(),
                    input.idempotency_key,
                    input.fingerprint_key_id,
                    input.request_fingerprint.as_slice(),
                    input.now_ms,
                    input.expires_at_ms,
                    input.resource_id.to_string(),
                ],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO resources
                 (id, account_id, kind, name, state, availability,
                  availability_code, spec_generation, driver_schema_version,
                  created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'creating', 'healthy', NULL,
                         1, ?5, ?6, ?6, NULL)",
                params![
                    input.resource_id.to_string(),
                    input.account_id.to_string(),
                    input.kind.as_str(),
                    input.name,
                    i64::from(input.driver_schema_version),
                    input.now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                input.account_id,
                "resource.create",
                "resource",
                &input.resource_id.to_string(),
                input.request_id,
                b"{\"state\":\"creating\"}",
                input.now_ms,
            )?;
            read_resource_tx(tx, input.account_id, input.resource_id)
                .map(ResourceCreateReservation::Reserved)
        })
    }

    /// Mark an owned resource-create idempotency row complete.
    pub fn complete_create(
        self,
        account_id: AccountId,
        key: &str,
        fingerprint: &[u8; 32],
        resource_id: ResourceId,
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency
                     SET state = 'complete', response_json = ?1
                     WHERE account_id = ?2 AND scope = 'resource.create'
                       AND idempotency_key = ?3 AND request_fingerprint = ?4
                       AND resource_id = ?5 AND state = 'running'",
                    params![
                        response,
                        account_id.to_string(),
                        key,
                        fingerprint.as_slice(),
                        resource_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "resource idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Tombstone a create with no physical effect and persist its stable failure for replay.
    #[allow(clippy::too_many_arguments)]
    pub fn fail_create(
        self,
        account_id: AccountId,
        key: &str,
        fingerprint: &[u8; 32],
        resource_id: ResourceId,
        code: ErrorCode,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let response = serde_json::to_vec(&serde_json::json!({ "code": code.as_str() }))
            .map_err(|_| resource_invariant())?;
        self.db.with_immediate(|tx| {
            let resource = read_resource_tx(tx, account_id, resource_id)?;
            if resource.state != ResourceState::Creating || has_referrers(tx, resource_id)? {
                return Err(resource_invariant());
            }
            let deleting_changed = tx
                .execute(
                    "UPDATE resources SET state = 'deleting', updated_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'creating'",
                    params![now_ms, resource_id.to_string(), account_id.to_string(),],
                )
                .map_err(|_| db_error())?;
            let resource_changed = tx
                .execute(
                    "UPDATE resources SET state = 'tombstoned', updated_at_ms = ?1,
                            deleted_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'deleting'",
                    params![now_ms, resource_id.to_string(), account_id.to_string(),],
                )
                .map_err(|_| db_error())?;
            let operation_changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'failed', response_json = ?1
                     WHERE account_id = ?2 AND scope = 'resource.create'
                       AND idempotency_key = ?3 AND request_fingerprint = ?4
                       AND resource_id = ?5 AND state = 'running'",
                    params![
                        response,
                        account_id.to_string(),
                        key,
                        fingerprint.as_slice(),
                        resource_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if deleting_changed != 1 || resource_changed != 1 || operation_changed != 1 {
                return Err(resource_invariant());
            }
            audit(
                tx,
                account_id,
                "resource.create_failed",
                "resource",
                &resource_id.to_string(),
                request_id,
                &response,
                now_ms,
            )
        })
    }

    /// Reserve or replay one account-scoped resource deletion.
    pub fn reserve_delete(
        &self,
        input: &ReserveResourceDelete<'_>,
    ) -> Result<ResourceDeleteReservation, PlatformError> {
        validate_idempotency_key(input.idempotency_key)?;
        if input.expires_at_ms <= input.now_ms {
            return Err(resource_invariant());
        }
        self.db.with_immediate(|tx| {
            let resource = read_resource_tx(tx, input.account_id, input.resource_id)?;
            let existing: Option<ExistingCreate> = tx
                .query_row(
                    "SELECT request_fingerprint, state, response_json, resource_id
                     FROM control_idempotency
                     WHERE account_id = ?1 AND scope = 'resource.delete'
                       AND idempotency_key = ?2",
                    params![input.account_id.to_string(), input.idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((fingerprint, state, response, stored_resource)) = existing {
                if fingerprint.as_slice() != input.request_fingerprint
                    || stored_resource.as_deref() != Some(&input.resource_id.to_string())
                {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "idempotency key fingerprint does not match",
                    ));
                }
                return match state.as_str() {
                    "complete" => response
                        .map(ResourceDeleteReservation::Complete)
                        .ok_or_else(resource_invariant),
                    "failed" => response
                        .map(ResourceDeleteReservation::Failed)
                        .ok_or_else(resource_invariant),
                    "running" => Ok(ResourceDeleteReservation::Continue(resource)),
                    _ => Err(resource_invariant()),
                };
            }
            tx.execute(
                "INSERT INTO control_idempotency
                 (account_id, scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, deployment_id, state,
                  created_at_ms, expires_at_ms, resource_id)
                 VALUES (?1, 'resource.delete', ?2, ?3, ?4, NULL, NULL,
                         'running', ?5, ?6, ?7)",
                params![
                    input.account_id.to_string(),
                    input.idempotency_key,
                    input.fingerprint_key_id,
                    input.request_fingerprint.as_slice(),
                    input.now_ms,
                    input.expires_at_ms,
                    input.resource_id.to_string(),
                ],
            )
            .map_err(|_| db_error())?;
            Ok(ResourceDeleteReservation::Reserved(resource))
        })
    }

    /// Mark an owned resource-delete idempotency row complete.
    pub fn complete_delete(
        self,
        account_id: AccountId,
        key: &str,
        fingerprint: &[u8; 32],
        resource_id: ResourceId,
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency
                     SET state = 'complete', response_json = ?1
                     WHERE account_id = ?2 AND scope = 'resource.delete'
                       AND idempotency_key = ?3 AND request_fingerprint = ?4
                       AND resource_id = ?5 AND state = 'running'",
                    params![
                        response,
                        account_id.to_string(),
                        key,
                        fingerprint.as_slice(),
                        resource_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "resource idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Read one resource while hiding cross-account existence.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<ResourceRecord, PlatformError> {
        self.db
            .with_read(|conn| read_resource_conn(conn, account_id, resource_id))
    }

    /// List account resources, optionally restricted to one kind.
    pub fn list(
        &self,
        account_id: AccountId,
        kind: Option<BindingKind>,
    ) -> Result<Vec<ResourceRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, account_id, kind, name, state, availability,
                            availability_code, spec_generation, driver_schema_version,
                            created_at_ms, updated_at_ms, deleted_at_ms
                     FROM resources
                     WHERE account_id = ?1 AND (?2 IS NULL OR kind = ?2)
                     ORDER BY kind, name, id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map(
                    params![account_id.to_string(), kind.map(BindingKind::as_str)],
                    map_resource,
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Rename only the display name without changing physical identity.
    pub fn rename(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<ResourceRecord, PlatformError> {
        validate_name(name)?;
        self.db.with_immediate(|tx| {
            let current = read_resource_tx(tx, account_id, resource_id)?;
            if current.state == ResourceState::Tombstoned {
                return Err(resource_not_ready());
            }
            let conflict: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM resources
                     WHERE account_id = ?1 AND kind = ?2 AND name = ?3
                       AND state != 'tombstoned' AND id != ?4)",
                    params![
                        account_id.to_string(),
                        current.kind.as_str(),
                        name,
                        resource_id.to_string(),
                    ],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if conflict {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNameConflict,
                    "a live resource already owns this name",
                ));
            }
            tx.execute(
                "UPDATE resources SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![name, now_ms, resource_id.to_string()],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "resource.rename",
                "resource",
                &resource_id.to_string(),
                request_id,
                b"{}",
                now_ms,
            )?;
            read_resource_tx(tx, account_id, resource_id)
        })
    }

    /// Persist a probe-derived availability state for one ready resource.
    pub fn set_availability(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        availability: ResourceAvailability,
        code: Option<&str>,
        now_ms: i64,
    ) -> Result<ResourceRecord, PlatformError> {
        if (availability == ResourceAvailability::Healthy) != code.is_none()
            || code.is_some_and(|value| {
                value.is_empty()
                    || value.len() > 128
                    || value.bytes().any(|byte| byte.is_ascii_control())
            })
        {
            return Err(resource_invariant());
        }
        self.db.with_immediate(|tx| {
            let current = read_resource_tx(tx, account_id, resource_id)?;
            if current.state != ResourceState::Ready {
                return Err(resource_not_ready());
            }
            tx.execute(
                "UPDATE resources
                 SET availability = ?1, availability_code = ?2, updated_at_ms = ?3
                 WHERE id = ?4",
                params![availability.as_str(), code, now_ms, resource_id.to_string()],
            )
            .map_err(|_| db_error())?;
            read_resource_tx(tx, account_id, resource_id)
        })
    }

    /// Transition a creating resource to ready after driver verification.
    pub fn mark_ready(&self, resource_id: ResourceId, now_ms: i64) -> Result<(), PlatformError> {
        self.transition(
            resource_id,
            ResourceState::Creating,
            ResourceState::Ready,
            now_ms,
        )
    }

    /// Atomically recheck referrers and enter the deleting lifecycle.
    pub fn begin_delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let current = read_resource_tx(tx, account_id, resource_id)?;
            if current.state == ResourceState::Deleting {
                return Ok(());
            }
            if !matches!(
                current.state,
                ResourceState::Ready | ResourceState::Creating
            ) {
                return Err(resource_not_ready());
            }
            if has_referrers(tx, resource_id)? {
                return Err(PlatformError::new(
                    ErrorCode::ResourceReferenced,
                    "resource still has retained referrers",
                ));
            }
            tx.execute(
                "UPDATE resources SET state = 'deleting', updated_at_ms = ?1
                 WHERE id = ?2",
                params![now_ms, resource_id.to_string()],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Permanently tombstone a deleting identity after driver deletion.
    pub fn mark_tombstoned(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            read_resource_tx(tx, account_id, resource_id)?;
            let changed = tx
                .execute(
                    "UPDATE resources
                     SET state = 'tombstoned', deleted_at_ms = ?1, updated_at_ms = ?1
                     WHERE id = ?2 AND state = 'deleting'",
                    params![now_ms, resource_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(resource_not_ready());
            }
            audit(
                tx,
                account_id,
                "resource.delete",
                "resource",
                &resource_id.to_string(),
                request_id,
                b"{\"state\":\"tombstoned\"}",
                now_ms,
            )?;
            Ok(())
        })
    }

    /// Return durable creating/deleting rows for startup reconciliation.
    pub fn reconcile_candidates(&self) -> Result<Vec<ResourceRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, account_id, kind, name, state, availability,
                            availability_code, spec_generation, driver_schema_version,
                            created_at_ms, updated_at_ms, deleted_at_ms
                     FROM resources WHERE state IN ('creating', 'deleting')
                     ORDER BY state, updated_at_ms, id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([], map_resource)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Read every typed delete blocker for one resource.
    pub fn referrers(
        &self,
        resource_id: ResourceId,
    ) -> Result<Vec<ResourceReferrer>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT resource_id, referrer_kind, referrer_id, created_at_ms
                     FROM resource_referrers WHERE resource_id = ?1
                     ORDER BY referrer_kind, referrer_id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([resource_id.to_string()], map_referrer)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    fn transition(
        self,
        resource_id: ResourceId,
        expected: ResourceState,
        target: ResourceState,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE resources SET state = ?1, updated_at_ms = ?2
                     WHERE id = ?3 AND state = ?4",
                    params![
                        target.as_str(),
                        now_ms,
                        resource_id.to_string(),
                        expected.as_str(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(resource_not_ready());
            }
            Ok(())
        })
    }
}

fn read_resource_conn(
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
        "SELECT EXISTS(SELECT 1 FROM resource_referrers WHERE resource_id = ?1)",
        [resource_id.to_string()],
        |row| row.get(0),
    )
    .map_err(|_| db_error())
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

#[allow(clippy::too_many_arguments)]
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
