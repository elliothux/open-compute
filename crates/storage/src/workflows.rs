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
    /// The reservation is account/name scoped and freezes the first class selection across
    /// upload retries and process restarts. Ready definitions are returned unchanged; binding
    /// admission separately proves their current ready version selects the exact class.
    pub fn reserve_definition(
        &self,
        account: AccountId,
        name: &str,
        class_name: &str,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        validate_class_name(class_name)?;
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
                return match existing.state {
                    ResourceState::Ready => Ok(existing),
                    ResourceState::Creating
                        if existing.reserved_class_name.as_deref() == Some(class_name) =>
                    {
                        Ok(existing)
                    }
                    ResourceState::Creating if existing.reserved_class_name.is_none() => {
                        let changed = tx
                            .execute(
                                "UPDATE workflow_definitions SET reserved_class_name=?2,updated_at_ms=?3
                                 WHERE id=?1 AND state='creating' AND reserved_class_name IS NULL",
                                params![existing.id.to_string(), class_name, now_ms],
                            )
                            .map_err(sql_error)?;
                        if changed != 1 {
                            return Err(error(ErrorCode::WorkflowNameConflict));
                        }
                        tx.query_row(
                            &format!("{DEFINITION_SELECT} WHERE id=?1"),
                            [existing.id.to_string()],
                            definition_row,
                        )
                        .map_err(sql_error)
                    }
                    _ => Err(error(ErrorCode::WorkflowNameConflict)),
                };
            }
            let id = WorkflowId::generate();
            tx.execute(
                "INSERT INTO workflow_definitions(id,account_id,name,state,availability,availability_code,
                 lifecycle_generation,reserved_class_name,created_at_ms,updated_at_ms)
                 VALUES(?1,?2,?3,'creating','degraded','WORKFLOW_VERSION_NOT_READY',1,?4,?5,?5)",
                params![id.to_string(), account.to_string(), name, class_name, now_ms],
            )
            .map_err(sql_error)?;
            tx.query_row(
                &format!("{DEFINITION_SELECT} WHERE id=?1"),
                [id.to_string()],
                definition_row,
            )
            .map_err(sql_error)
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
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_referrers WHERE definition_id=?1)
                OR EXISTS(SELECT 1 FROM workflow_versions WHERE definition_id=?1 AND state IN ('staging','validating'))",
                [id.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            tx.execute("UPDATE workflow_definitions SET state='deleting',current_version_id=NULL,updated_at_ms=?2 WHERE id=?1",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_versions SET state='deleting' WHERE definition_id=?1 AND state IN ('ready','rejected')",
                [id.to_string()]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_versions SET state='tombstoned',deleted_at_ms=?2 WHERE definition_id=?1 AND state='deleting'",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_definitions SET state='tombstoned',deleted_at_ms=?2,updated_at_ms=?2 WHERE id=?1",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            tx.query_row(
                &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(), id.to_string()],
                definition_row,
            )
            .map_err(sql_error)
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
