//! Workflow catalog, version, binding, and live-version authority.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort, ControlDb, VersionState,
};
use open_compute_core::{
    AccountId, BindingId, ErrorCode, PlatformError, ResourceAvailability, ResourceState, VersionId,
    WorkflowId, WorkflowInstanceId, WorkflowOperationId, WorkflowToken, WorkflowVersionId,
};
use rusqlite::{OptionalExtension as _, params, params_from_iter};
use sha2::{Digest as _, Sha256};

pub(crate) mod bindings;
pub(crate) mod helpers;
mod instances;
pub(crate) mod integrity;
mod model;
pub(crate) mod operations;
mod versions;
use helpers::*;
pub use model::*;
pub use operations::{
    WorkflowAppliedOperation, WorkflowGcAcknowledgement, WorkflowGcReceipt, WorkflowOperation,
    WorkflowOperationKind, WorkflowOperationResult, WorkflowRejectedOperation,
};

#[cfg(test)]
#[path = "workflows/workflow_tests.rs"]
mod tests;

/// Short synchronous transactions over the platform-owned control authority.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> WorkflowRepository<'a> {
    /// Borrow the control database owned by ocd.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Reserve an account-scoped logical definition before validating its first version.
    pub fn create_definition(
        &self,
        account: AccountId,
        name: &str,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        self.db.with_immediate(|tx| {
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND deleted_at_ms IS NULL)",
                [account.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNotFound));
            }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE account_id=?1 AND name=?2 AND state!='tombstoned')",
                params![account.to_string(),name], |row| row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNameConflict));
            }
            let id = WorkflowId::generate();
            tx.execute("INSERT INTO workflow_definitions(id,account_id,name,state,availability,availability_code,
                lifecycle_generation,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'creating','degraded','WORKFLOW_VERSION_NOT_READY',1,?4,?4)",
                params![id.to_string(),account.to_string(),name,now_ms]).map_err(sql_error)?;
            tx.query_row(&format!("{DEFINITION_SELECT} WHERE id=?1"), [id.to_string()], definition_row).map_err(sql_error)
        })
    }

    /// Reserve or reuse the creating definition required by Wrangler's upload-before-PUT flow.
    ///
    /// The reservation is account/name scoped and freezes class selection across retries and
    /// process restarts. Reclaiming the same class advances the fence; a different pending class
    /// fails closed. Ready definitions retain their current runtime version while staging the
    /// pending class.
    pub fn reserve_definition(
        &self,
        account: AccountId,
        name: &str,
        class_name: &str,
        owner: &str,
        now_ms: i64,
    ) -> Result<WorkflowDefinitionReservation, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        validate_class_name(class_name)?;
        if owner.is_empty() || owner.len() > 128 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            if !tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND deleted_at_ms IS NULL)",
                    [account.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?
            {
                return Err(error(ErrorCode::WorkflowNotFound));
            }
            let existing = tx
                .query_row(
                    &format!(
                        "{DEFINITION_SELECT} WHERE account_id=?1 AND name=?2 AND state!='tombstoned'"
                    ),
                    params![account.to_string(), name],
                    definition_row,
                )
                .optional()
                .map_err(sql_error)?;
            if let Some(existing) = existing {
                if !matches!(existing.state, ResourceState::Creating | ResourceState::Ready) {
                    return Err(error(ErrorCode::WorkflowNameConflict));
                }
                if existing
                    .reserved_class_name
                    .as_deref()
                    .is_some_and(|reserved| reserved != class_name)
                {
                    return Err(error(ErrorCode::WorkflowNameConflict));
                }
                if existing.reserved_class_name.as_deref() == Some(class_name)
                    && existing.reservation_owner.as_deref() == Some(owner)
                {
                    return reservation(existing);
                }
                let fence = existing
                    .reservation_fence
                    .checked_add(1)
                    .ok_or_else(invariant)?;
                let changed = tx
                    .execute(
                        "UPDATE workflow_definitions SET reserved_class_name=?2,reservation_owner=?3,
                         reservation_fence=?4,reservation_state='reserved',reservation_created_definition=0,
                         updated_at_ms=?5 WHERE id=?1 AND state IN ('creating','ready')",
                        params![existing.id.to_string(), class_name, owner, fence, now_ms],
                    )
                    .map_err(sql_error)?;
                if changed != 1 {
                    return Err(error(ErrorCode::WorkflowNameConflict));
                }
                let definition = tx
                    .query_row(
                        &format!("{DEFINITION_SELECT} WHERE id=?1"),
                        [existing.id.to_string()],
                        definition_row,
                    )
                    .map_err(sql_error)?;
                return reservation(definition);
            }
            let id = WorkflowId::generate();
            tx.execute(
                "INSERT INTO workflow_definitions(id,account_id,name,state,availability,availability_code,
                 lifecycle_generation,reserved_class_name,reservation_owner,reservation_fence,reservation_state,
                 reservation_created_definition,created_at_ms,updated_at_ms)
                 VALUES(?1,?2,?3,'creating','degraded','WORKFLOW_VERSION_NOT_READY',1,?4,?5,1,'reserved',1,?6,?6)",
                params![id.to_string(), account.to_string(), name, class_name, owner, now_ms],
            )
            .map_err(sql_error)?;
            let definition = tx
                .query_row(
                    &format!("{DEFINITION_SELECT} WHERE id=?1"),
                    [id.to_string()],
                    definition_row,
                )
                .map_err(sql_error)?;
            reservation(definition)
        })
    }

    /// Release only the exact still-unconsumed reservation owned by this operation.
    pub fn release_definition_reservation(
        &self,
        account: AccountId,
        reservation: &WorkflowDefinitionReservation,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let current = tx
                .query_row(
                    &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                    params![account.to_string(), reservation.definition.id.to_string()],
                    definition_row,
                )
                .optional()
                .map_err(sql_error)?;
            let Some(current) = current else {
                return Ok(false);
            };
            if current.reservation_owner.as_deref() != Some(reservation.owner.as_str())
                || current.reservation_fence != reservation.fence
            {
                return Ok(false);
            }
            let active_consumer = reservation_has_active_consumer(tx, &current)?;
            if active_consumer {
                return Ok(false);
            }
            if reservation.created_definition
                && current.reservation_created_definition == Some(true)
                && current.state == ResourceState::Creating
                && current.current_version_id.is_none()
            {
                let has_any_consumers = tx
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM workflow_bindings WHERE definition_id=?1
                            UNION ALL
                            SELECT 1 FROM workflow_versions WHERE definition_id=?1
                        )",
                        [current.id.to_string()],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(sql_error)?;
                if has_any_consumers {
                    return Ok(false);
                }
                let deleting = tx
                    .execute(
                        "UPDATE workflow_definitions SET state='deleting',reserved_class_name=NULL,
                         reservation_owner=NULL,reservation_state=NULL,reservation_created_definition=NULL,
                         delete_fence=delete_fence+1,updated_at_ms=?4
                         WHERE account_id=?1 AND id=?2 AND reservation_owner=?3 AND reservation_fence=?5
                         AND state='creating' AND current_version_id IS NULL",
                        params![
                            account.to_string(),
                            current.id.to_string(),
                            reservation.owner,
                            now_ms,
                            reservation.fence
                        ],
                    )
                    .map_err(sql_error)?;
                if deleting != 1 {
                    return Ok(false);
                }
                let tombstoned = tx
                    .execute(
                        "UPDATE workflow_definitions SET state='tombstoned',updated_at_ms=?3,deleted_at_ms=?3
                         WHERE account_id=?1 AND id=?2 AND state='deleting'",
                        params![account.to_string(), current.id.to_string(), now_ms],
                    )
                    .map_err(sql_error)?;
                return Ok(tombstoned == 1);
            }
            if matches!(current.state, ResourceState::Creating | ResourceState::Ready) {
                let changed = tx
                    .execute(
                        "UPDATE workflow_definitions SET reserved_class_name=NULL,reservation_owner=NULL,
                         reservation_state=NULL,reservation_created_definition=NULL,updated_at_ms=?5
                         WHERE account_id=?1 AND id=?2 AND reservation_owner=?3 AND reservation_fence=?4
                         AND state IN ('creating','ready')",
                        params![
                            account.to_string(),
                            current.id.to_string(),
                            reservation.owner,
                            reservation.fence,
                            now_ms
                        ],
                    )
                    .map_err(sql_error)?;
                return Ok(changed == 1);
            }
            Ok(false)
        })
    }

    /// Read a definition only inside its owning account, including retained tombstones.
    pub fn definition(
        &self,
        account: AccountId,
        id: WorkflowId,
    ) -> Result<WorkflowDefinition, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(), id.to_string()],
                definition_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowNotFound))
        })
    }

    /// Bounded, filtered, and sorted account-scoped catalog listing.
    #[allow(clippy::too_many_arguments)]
    pub fn definitions(
        &self,
        account: AccountId,
        search: Option<&str>,
        status: Option<ResourceState>,
        sort: CatalogSort,
        direction: CatalogDirection,
        after: Option<CatalogCursor>,
        limit: u16,
    ) -> Result<CatalogListPage<WorkflowDefinition>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let exact_id = search.and_then(crate::search_as_workflow_id);
        let search_needle = if exact_id.is_some() {
            None
        } else {
            search.map(str::to_lowercase)
        };
        let fetch = u32::from(limit).saturating_add(1);
        let query = build_catalog_sql(
            &format!("{DEFINITION_SELECT} WHERE account_id = ? AND state != 'tombstoned'"),
            CatalogColumns {
                id: "id",
                name: "name",
                state: "state",
                created_at: "created_at_ms",
                updated_at: "updated_at_ms",
            },
            account.to_string(),
            search_needle,
            exact_id.map(|id| id.to_string()),
            status.map(|value| value.as_str().to_string()),
            sort,
            direction,
            after,
            fetch,
        )?;
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(&query.text).map_err(sql_error)?;
            let rows = statement
                .query_map(params_from_iter(query.values), definition_row)
                .map_err(sql_error)?;
            let mut definitions = rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?;
            let next_cursor = if definitions.len() > usize::from(limit) {
                definitions.pop();
                definitions.last().map(|definition| {
                    record_catalog_cursor(
                        sort,
                        direction,
                        &definition.name,
                        definition.created_at_ms,
                        definition.updated_at_ms,
                        &definition.id.to_string(),
                    )
                })
            } else {
                None
            };
            Ok(CatalogListPage {
                items: definitions,
                next_cursor,
            })
        })
    }

    /// Rename display identity without changing existing instance events or bindings.
    pub fn rename(
        &self,
        account: AccountId,
        id: WorkflowId,
        name: &str,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        self.db.with_immediate(|tx| {
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE account_id=?1 AND name=?2 AND id!=?3 AND state!='tombstoned')",
                params![account.to_string(),name,id.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNameConflict));
            }
            let changed = tx.execute("UPDATE workflow_definitions SET name=?3,updated_at_ms=?4 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')",
                params![account.to_string(),id.to_string(),name,now_ms]).map_err(sql_error)?;
            if changed != 1 { return Err(error(ErrorCode::WorkflowNotReady)); }
            tx.query_row(
                &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(), id.to_string()],
                definition_row,
            )
            .map_err(sql_error)
        })
    }

    /// Tombstone only after the unified registry and pending validations are empty.
    pub fn delete(
        &self,
        account: AccountId,
        id: WorkflowId,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        self.db.with_immediate(|tx| {
            let definition = tx.query_row(&format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(),id.to_string()],definition_row).optional().map_err(sql_error)?
                .ok_or_else(||error(ErrorCode::WorkflowNotFound))?;
            if definition.state == ResourceState::Tombstoned {
                return Ok(definition);
            }
            if definition.reservation_owner.is_some() {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_referrers WHERE definition_id=?1)
                OR EXISTS(SELECT 1 FROM workflow_versions WHERE definition_id=?1 AND state IN ('staging','validating'))",
                [id.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            let fence = if definition.state == ResourceState::Deleting {
                definition.delete_fence
            } else {
                let changed = tx.execute(
                    "UPDATE workflow_definitions SET state='deleting',delete_fence=delete_fence+1,
                     updated_at_ms=?3 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')",
                    params![account.to_string(),id.to_string(),now_ms],
                ).map_err(sql_error)?;
                if changed != 1 { return Err(error(ErrorCode::WorkflowNotReady)); }
                definition.delete_fence.checked_add(1).ok_or_else(invariant)?
            };
            finalize_definition_delete(tx, account, id, fence, now_ms)
        })
    }

    /// Atomically fence new reservations before asynchronous instance cleanup begins.
    pub fn begin_definition_delete(
        &self,
        account: AccountId,
        name: &str,
        now_ms: i64,
    ) -> Result<WorkflowDeleteIntent, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        self.db.with_immediate(|tx| {
            let definition = tx
                .query_row(
                    &format!(
                        "{DEFINITION_SELECT} WHERE account_id=?1 AND name=?2
                     AND state IN ('ready','deleting')"
                    ),
                    params![account.to_string(), name],
                    definition_row,
                )
                .optional()
                .map_err(sql_error)?
                .ok_or_else(|| error(ErrorCode::WorkflowNotFound))?;
            if definition.state == ResourceState::Deleting {
                return delete_intent(definition);
            }
            let id = definition.id;
            if definition.reservation_owner.is_some()
                || tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM workflow_referrers
                       WHERE definition_id=?1 AND referrer_kind='binding')
                     OR EXISTS(SELECT 1 FROM workflow_versions
                       WHERE definition_id=?1 AND state IN ('staging','validating'))",
                        [id.to_string()],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(sql_error)?
            {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            let fence = definition
                .delete_fence
                .checked_add(1)
                .ok_or_else(invariant)?;
            let changed = tx.execute(
                "UPDATE workflow_definitions SET state='deleting',delete_fence=?3,updated_at_ms=?4
                 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')
                   AND reservation_owner IS NULL AND delete_fence=?5",
                params![account.to_string(),id.to_string(),fence,now_ms,definition.delete_fence],
            ).map_err(sql_error)?;
            if changed != 1 {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            let claimed = tx
                .query_row(
                    &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                    params![account.to_string(), id.to_string()],
                    definition_row,
                )
                .map_err(sql_error)?;
            delete_intent(claimed)
        })
    }

    /// Finalize the exact durable delete intent after every instance referrer is gone.
    pub fn finish_definition_delete(
        &self,
        account: AccountId,
        intent: &WorkflowDeleteIntent,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        if intent.definition.account_id != account {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            finalize_definition_delete(tx, account, intent.definition.id, intent.fence, now_ms)
        })
    }

    /// Fence admission after an authority mismatch; recovery must prove integrity before clearing it.
    pub fn mark_unavailable(
        &self,
        account: AccountId,
        id: WorkflowId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx.execute("UPDATE workflow_definitions SET availability='unavailable',availability_code='WORKFLOW_INVARIANT_VIOLATION',
                updated_at_ms=?3 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')",
                params![account.to_string(),id.to_string(),now_ms]).map_err(sql_error)?;
            if changed != 1 { return Err(error(ErrorCode::WorkflowNotFound)); }
            Ok(())
        })
    }
}

fn reservation(
    definition: WorkflowDefinition,
) -> Result<WorkflowDefinitionReservation, PlatformError> {
    let owner = definition.reservation_owner.clone().ok_or_else(invariant)?;
    let created_definition = definition
        .reservation_created_definition
        .ok_or_else(invariant)?;
    if definition.reserved_class_name.is_none()
        || definition.reservation_fence < 1
        || definition.reservation_state.is_none()
    {
        return Err(invariant());
    }
    Ok(WorkflowDefinitionReservation {
        fence: definition.reservation_fence,
        definition,
        owner,
        created_definition,
    })
}

fn reservation_has_active_consumer(
    tx: &rusqlite::Transaction<'_>,
    definition: &WorkflowDefinition,
) -> Result<bool, PlatformError> {
    let owner = definition
        .reservation_owner
        .as_deref()
        .ok_or_else(invariant)?;
    if definition.reservation_fence < 1 {
        return Err(invariant());
    }
    tx.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM workflow_bindings b JOIN worker_versions v ON v.id=b.version_id
              WHERE b.reservation_owner=?1 AND b.reservation_fence=?2
                AND b.definition_id=?3 AND v.state IN ('staging','validating','ready')
            UNION ALL
            SELECT 1 FROM workflow_versions WHERE reservation_owner=?1
              AND reservation_fence=?2 AND definition_id=?3
              AND state IN ('staging','validating','ready')
        )",
        params![
            owner,
            definition.reservation_fence,
            definition.id.to_string()
        ],
        |row| row.get::<_, bool>(0),
    )
    .map_err(sql_error)
}

fn delete_intent(definition: WorkflowDefinition) -> Result<WorkflowDeleteIntent, PlatformError> {
    if definition.state != ResourceState::Deleting || definition.delete_fence < 1 {
        return Err(invariant());
    }
    let fence = definition.delete_fence;
    Ok(WorkflowDeleteIntent { definition, fence })
}

fn finalize_definition_delete(
    tx: &rusqlite::Transaction<'_>,
    account: AccountId,
    id: WorkflowId,
    fence: i64,
    now_ms: i64,
) -> Result<WorkflowDefinition, PlatformError> {
    if fence < 1 {
        return Err(invariant());
    }
    let definition = tx
        .query_row(
            &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
            params![account.to_string(), id.to_string()],
            definition_row,
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| error(ErrorCode::WorkflowNotFound))?;
    if definition.state == ResourceState::Tombstoned && definition.delete_fence == fence {
        return Ok(definition);
    }
    if definition.state != ResourceState::Deleting || definition.delete_fence != fence {
        return Err(error(ErrorCode::WorkflowNotReady));
    }
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM workflow_referrers WHERE definition_id=?1)
         OR EXISTS(SELECT 1 FROM workflow_versions WHERE definition_id=?1 AND state IN ('staging','validating'))",
        [id.to_string()],
        |row| row.get::<_,bool>(0),
    ).map_err(sql_error)? {
        return Err(error(ErrorCode::WorkflowReferenced));
    }
    tx.execute(
        "UPDATE workflow_definitions SET current_version_id=NULL,updated_at_ms=?3
         WHERE account_id=?1 AND id=?2 AND state='deleting' AND delete_fence=?4",
        params![account.to_string(), id.to_string(), now_ms, fence],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE workflow_versions SET state='deleting'
         WHERE definition_id=?1 AND state IN ('ready','rejected')",
        [id.to_string()],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE workflow_versions SET state='tombstoned',deleted_at_ms=?2
         WHERE definition_id=?1 AND state='deleting'",
        params![id.to_string(), now_ms],
    )
    .map_err(sql_error)?;
    let changed = tx
        .execute(
            "UPDATE workflow_definitions SET state='tombstoned',deleted_at_ms=?3,updated_at_ms=?3
         WHERE account_id=?1 AND id=?2 AND state='deleting' AND delete_fence=?4",
            params![account.to_string(), id.to_string(), now_ms, fence],
        )
        .map_err(sql_error)?;
    if changed != 1 {
        return Err(invariant());
    }
    tx.query_row(
        &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
        params![account.to_string(), id.to_string()],
        definition_row,
    )
    .map_err(sql_error)
}
