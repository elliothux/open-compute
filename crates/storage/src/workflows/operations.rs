//! Exact cross-database restart/purge intents and proof-gated control finalization.

use super::*;
use open_compute_core::workflow::{WorkflowRestartSelector, WorkflowRestartStepType};
use open_compute_core::{WorkflowOperationId, WorkflowsConfig};

#[path = "operation_evidence.rs"]
mod evidence;
pub use evidence::{
    WorkflowGcAcknowledgement, WorkflowGcReceipt, WorkflowOperationResult,
    WorkflowRejectedOperation,
};

/// Mutually exclusive cross-database operations for one internal instance identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowOperationKind {
    /// Reset execution under the same immutable version and a fresh generation.
    Restart,
    /// Remove expired terminal history and release its external identity.
    Purge,
}

impl WorkflowOperationKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Purge => "purge",
        }
    }
}

/// A committed control intent. No public constructor or wire deserializer can forge authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowOperation {
    pub(crate) id: WorkflowOperationId,
    pub(crate) identity: WorkflowInstanceIdentity,
    pub(crate) kind: WorkflowOperationKind,
    pub(crate) restart_from: Option<WorkflowRestartSelector>,
    pub(crate) target_generation: i64,
    pub(crate) prior_state: WorkflowRefState,
    pub(crate) created_at_ms: i64,
    pub(crate) sequence: i64,
}

impl WorkflowOperation {
    /// Stable operation correlation identity, reused during reconciliation.
    #[must_use]
    pub const fn id(&self) -> WorkflowOperationId {
        self.id
    }
    /// Frozen instance identity before the operation, including its expected generation.
    #[must_use]
    pub const fn identity(&self) -> &WorkflowInstanceIdentity {
        &self.identity
    }
    /// Requested transition.
    #[must_use]
    pub const fn kind(&self) -> WorkflowOperationKind {
        self.kind
    }
    /// Exact restart selector frozen by control authority; absent means restart from the beginning.
    #[must_use]
    pub const fn restart_from(&self) -> Option<&WorkflowRestartSelector> {
        self.restart_from.as_ref()
    }
    /// Exact generation after application; purge keeps the original generation.
    #[must_use]
    pub const fn target_generation(&self) -> i64 {
        self.target_generation
    }
    /// Durable intent timestamp for bounded reconcile/age diagnostics.
    #[must_use]
    pub const fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
    /// Monotonic operation sequence, independent of execution generation and wall time.
    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }
}

/// Evidence read from a committed scheduler restart marker or GC receipt.
/// Only the storage crate can construct this value; absence alone never proves deletion.
#[derive(Clone, Debug)]
pub struct WorkflowAppliedOperation {
    pub(crate) operation: WorkflowOperation,
}

impl WorkflowAppliedOperation {
    /// Exact control intent whose scheduler application was proved.
    #[must_use]
    pub const fn operation(&self) -> &WorkflowOperation {
        &self.operation
    }
}

impl WorkflowRepository<'_> {
    /// Aggregate pending saga metadata without reading identities, input or private proof material.
    pub fn inspect_operations(&self) -> Result<WorkflowOperationInspection, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row("SELECT coalesce(SUM(kind='restart'),0),coalesce(SUM(kind='purge'),0),MIN(created_at_ms)
                FROM workflow_instance_operations",[],|row|Ok(WorkflowOperationInspection {
                    pending_restarts:row.get(0)?,pending_purges:row.get(1)?,oldest_operation_at_ms:row.get(2)?,
                })).map_err(sql_error)
        })
    }

    /// Prepare one durable operation and reserve restart quota before touching scheduler state.
    /// The scheduler must independently recheck terminal expiry and the full frozen identity.
    pub fn prepare_instance_operation(
        &self,
        identity: &WorkflowInstanceIdentity,
        operation_id: WorkflowOperationId,
        kind: WorkflowOperationKind,
        limits: &WorkflowsConfig,
        now_ms: i64,
    ) -> Result<WorkflowOperation, PlatformError> {
        self.prepare_operation(identity, operation_id, kind, None, limits, now_ms)
    }

    /// Prepare an exact restart intent including the optional pinned step occurrence selector.
    pub fn prepare_restart_operation(
        &self,
        identity: &WorkflowInstanceIdentity,
        operation_id: WorkflowOperationId,
        restart_from: Option<WorkflowRestartSelector>,
        limits: &WorkflowsConfig,
        now_ms: i64,
    ) -> Result<WorkflowOperation, PlatformError> {
        if let Some(selector) = &restart_from {
            selector.validate()?;
        }
        self.prepare_operation(
            identity,
            operation_id,
            WorkflowOperationKind::Restart,
            restart_from,
            limits,
            now_ms,
        )
    }

    fn prepare_operation(
        self,
        identity: &WorkflowInstanceIdentity,
        operation_id: WorkflowOperationId,
        kind: WorkflowOperationKind,
        restart_from: Option<WorkflowRestartSelector>,
        limits: &WorkflowsConfig,
        now_ms: i64,
    ) -> Result<WorkflowOperation, PlatformError> {
        limits.validate()?;
        if identity.target.capability_version != 1
            || (kind == WorkflowOperationKind::Purge && restart_from.is_some())
        {
            return Err(error(ErrorCode::WorkflowMethodUnsupported));
        }
        self.db.with_immediate(|tx| {
            instances::verify_identity(tx, identity)?;
            if let Some(operation) = read_operation(tx, identity.instance_id)? {
                if operation.id != operation_id
                    || operation.identity != *identity
                    || operation.kind != kind
                    || operation.restart_from != restart_from
                {
                    return Err(error(ErrorCode::WorkflowInstanceBusy));
                }
                return Ok(operation);
            }
            let reservation = tx.query_row(&format!("{} WHERE r.instance_id=?1", instances::RESERVATION_SELECT),
                [identity.instance_id.to_string()], instances::reservation_row).map_err(sql_error)?;
            if !matches!(reservation.state, WorkflowRefState::Live | WorkflowRefState::Retained)
                || (kind == WorkflowOperationKind::Purge && reservation.state != WorkflowRefState::Retained)
            { return Err(error(ErrorCode::WorkflowInstanceStateConflict)); }
            if !instances::referrers_intact(tx, identity)? { return Err(invariant()); }
            if kind == WorkflowOperationKind::Restart {
                let ready: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_versions v
                    JOIN workflow_definitions f ON f.id=v.definition_id JOIN worker_deployments d ON d.id=v.deployment_id
                    JOIN workers w ON w.id=d.worker_id WHERE v.id=?1 AND v.state='ready' AND f.state='ready'
                      AND f.availability='healthy' AND d.state='ready' AND w.deleted_at_ms IS NULL)",
                    [identity.target.version_id.to_string()], |row|row.get(0)).map_err(sql_error)?;
                if !ready { return Err(error(ErrorCode::WorkflowVersionNotReady)); }
                if reservation.state == WorkflowRefState::Retained {
                    let active: u64 = tx.query_row("SELECT COUNT(*) FROM workflow_instance_referrers r
                        JOIN workflow_definitions f ON f.id=r.definition_id WHERE f.account_id=?1
                          AND r.state IN ('creating','live','restarting')", [identity.target.account_id.to_string()],
                        |row|row.get(0)).map_err(sql_error)?;
                    if active >= u64::from(limits.max_active_per_account) { return Err(error(ErrorCode::WorkflowStateQuotaExceeded)); }
                }
            }
            let target_generation = if kind == WorkflowOperationKind::Restart {
                identity.instance_generation.checked_add(1).ok_or_else(||error(ErrorCode::WorkflowInstanceStateConflict))?
            } else { identity.instance_generation };
            let sequence: i64=tx.query_row("SELECT operation_sequence FROM workflow_instance_referrers WHERE instance_id=?1",
                [identity.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
            let sequence=sequence.checked_add(1).ok_or_else(||error(ErrorCode::WorkflowInstanceStateConflict))?;
            tx.execute("UPDATE workflow_instance_referrers SET operation_sequence=?2 WHERE instance_id=?1",
                params![identity.instance_id.to_string(),sequence]).map_err(sql_error)?;
            let operation = WorkflowOperation {id: operation_id, identity: identity.clone(), kind, restart_from,
                target_generation, prior_state: reservation.state, created_at_ms: now_ms, sequence};
            tx.execute("INSERT INTO workflow_instance_operations(operation_id,instance_id,creation_nonce,expected_generation,
                target_generation,kind,restart_from_name,restart_from_count,restart_from_kind,prior_ref_state,created_at_ms,operation_sequence)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![
                operation.id.to_string(),identity.instance_id.to_string(),identity.creation_nonce.as_bytes().as_slice(),
                identity.instance_generation,target_generation,kind.as_str(),
                operation.restart_from.as_ref().map(|selector| selector.name.as_str()),
                operation.restart_from.as_ref().map(|selector| selector.count),
                operation.restart_from.as_ref().and_then(|selector| selector.step_type.map(WorkflowRestartStepType::as_str)),
                if reservation.state==WorkflowRefState::Live {"live"} else {"retained"},now_ms,sequence]).map_err(sql_error)?;
            if kind == WorkflowOperationKind::Restart {
                tx.execute("UPDATE workflow_instance_referrers SET state='restarting',updated_at_ms=?2 WHERE instance_id=?1",
                    params![identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
            }
            Ok(operation)
        })
    }

    /// Read the exact unfinished intent for a single internal identity.
    pub fn instance_operation(
        &self,
        id: WorkflowInstanceId,
    ) -> Result<Option<WorkflowOperation>, PlatformError> {
        self.db.with_read(|conn| read_operation(conn, id))
    }

    /// Page unfinished operations by stable UUID without scanning retained instance history.
    pub fn instance_operations(
        &self,
        after: Option<WorkflowOperationId>,
        limit: u32,
    ) -> Result<Vec<WorkflowOperation>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT instance_id FROM workflow_instance_operations
                WHERE operation_id>?1 ORDER BY operation_id LIMIT ?2",
                )
                .map_err(sql_error)?;
            let ids = statement
                .query_map(
                    params![after.map_or_else(String::new, |id| id.to_string()), limit],
                    |row| parse(row, 0),
                )
                .map_err(sql_error)?
                .collect::<Result<Vec<WorkflowInstanceId>, _>>()
                .map_err(sql_error)?;
            ids.into_iter()
                .map(|id| read_operation(conn, id)?.ok_or_else(invariant))
                .collect()
        })
    }

    /// Finalize only with an exact committed scheduler proof, preserving idempotence after lost replies.
    pub fn complete_instance_operation(
        &self,
        proof: &WorkflowAppliedOperation,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let operation = &proof.operation;
        self.db.with_immediate(|tx| {
            let current=read_operation(tx,operation.identity.instance_id)?;
            if let Some(current)=current {
                if current != *operation { return Err(error(ErrorCode::WorkflowInstanceBusy)); }
                tx.execute("UPDATE workflow_instance_operations SET applied=1 WHERE operation_id=?1",[operation.id.to_string()]).map_err(sql_error)?;
                match operation.kind {
                    WorkflowOperationKind::Restart => {
                        tx.execute("UPDATE workflow_instance_referrers SET state='live',instance_generation=?2,updated_at_ms=?3
                            WHERE instance_id=?1 AND state='restarting'",params![operation.identity.instance_id.to_string(),operation.target_generation,now_ms]).map_err(sql_error)?;
                        tx.execute("DELETE FROM workflow_instance_operations WHERE operation_id=?1",[operation.id.to_string()]).map_err(sql_error)?;
                    }
                    WorkflowOperationKind::Purge => {
                        tx.execute("UPDATE workflow_instance_referrers SET state='releasing',updated_at_ms=?2 WHERE instance_id=?1 AND state='retained'",
                            params![operation.identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
                        tx.execute("UPDATE workflow_instance_referrers SET state='released',released_at_ms=?2,updated_at_ms=?2
                            WHERE instance_id=?1 AND state='releasing'",params![operation.identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
                        tx.execute("DELETE FROM workflow_instance_referrers WHERE instance_id=?1",[operation.identity.instance_id.to_string()]).map_err(sql_error)?;
                    }
                }
            }
            let reservation = tx.query_row(&format!("{} WHERE r.instance_id=?1", instances::RESERVATION_SELECT),
                [operation.identity.instance_id.to_string()], instances::reservation_row).optional().map_err(sql_error)?;
            match (operation.kind,reservation) {
                (WorkflowOperationKind::Restart,Some(row)) => {
                    let mut expected=operation.identity.clone(); expected.instance_generation=operation.target_generation;
                    if row.identity!=expected || row.state!=WorkflowRefState::Live || !instances::referrers_intact(tx,&expected)? {
                        return Err(invariant());
                    }
                }
                (WorkflowOperationKind::Purge,None) => {
                    let dangling: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM deployment_referrers WHERE kind='workflow_instance' AND ref_id=?1)
                        OR EXISTS(SELECT 1 FROM workflow_referrers WHERE referrer_kind='instance' AND referrer_id=?1)",
                        [operation.identity.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
                    if dangling { return Err(invariant()); }
                }
                _ => return Err(invariant()),
            }
            Ok(())
        })
    }
}

fn read_operation(
    conn: &rusqlite::Connection,
    id: WorkflowInstanceId,
) -> Result<Option<WorkflowOperation>, PlatformError> {
    let raw=conn.query_row("SELECT operation_id,creation_nonce,expected_generation,target_generation,kind,
        restart_from_name,restart_from_count,restart_from_kind,prior_ref_state,applied,created_at_ms,operation_sequence
        FROM workflow_instance_operations WHERE instance_id=?1",[id.to_string()],|row|Ok((
            parse::<WorkflowOperationId>(row,0)?,digest(row,1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?,row.get::<_,String>(4)?,
            row.get::<_,Option<String>>(5)?,row.get::<_,Option<u32>>(6)?,row.get::<_,Option<String>>(7)?,
            row.get::<_,String>(8)?,row.get::<_,bool>(9)?,row.get::<_,i64>(10)?,row.get::<_,i64>(11)?))).optional().map_err(sql_error)?;
    let Some((
        id,
        nonce,
        expected_generation,
        target_generation,
        kind,
        restart_from_name,
        restart_from_count,
        restart_from_kind,
        prior,
        applied,
        created_at_ms,
        sequence,
    )) = raw
    else {
        return Ok(None);
    };
    let row=conn.query_row(&format!("{} WHERE r.instance_id=(SELECT instance_id FROM workflow_instance_operations WHERE operation_id=?1)",instances::RESERVATION_SELECT),
        [id.to_string()],instances::reservation_row).map_err(sql_error)?;
    let kind = match kind.as_str() {
        "restart" => WorkflowOperationKind::Restart,
        "purge" => WorkflowOperationKind::Purge,
        _ => return Err(invariant()),
    };
    let restart_from = match (restart_from_name, restart_from_count, restart_from_kind) {
        (None, None, None) => None,
        (Some(name), Some(count), kind) => {
            let step_type = match kind.as_deref() {
                None => None,
                Some("do") => Some(WorkflowRestartStepType::Do),
                Some("sleep") => Some(WorkflowRestartStepType::Sleep),
                Some("waitForEvent") => Some(WorkflowRestartStepType::WaitForEvent),
                _ => return Err(invariant()),
            };
            let selector = WorkflowRestartSelector {
                name,
                count,
                step_type,
            };
            selector.validate().map_err(|_| invariant())?;
            Some(selector)
        }
        _ => return Err(invariant()),
    };
    let prior_state = match prior.as_str() {
        "live" => WorkflowRefState::Live,
        "retained" => WorkflowRefState::Retained,
        _ => return Err(invariant()),
    };
    let reserved_sequence: i64 = conn
        .query_row(
            "SELECT operation_sequence FROM workflow_instance_referrers WHERE instance_id=?1",
            [row.identity.instance_id.to_string()],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if applied
        || sequence < 1
        || sequence != reserved_sequence
        || row.identity.creation_nonce.as_bytes() != &nonce
        || row.identity.instance_generation != expected_generation
        || row.identity.target.capability_version != 1
        || version_digest(&row.identity.target)? != row.identity.target.descriptor_sha256
        || !instances::referrers_intact(conn, &row.identity)?
        || (kind == WorkflowOperationKind::Restart
            && (row.state != WorkflowRefState::Restarting
                || expected_generation.checked_add(1) != Some(target_generation)))
        || (kind == WorkflowOperationKind::Purge && restart_from.is_some())
        || (kind == WorkflowOperationKind::Purge
            && (row.state != WorkflowRefState::Retained
                || prior_state != WorkflowRefState::Retained
                || expected_generation != target_generation))
    {
        return Err(invariant());
    }
    Ok(Some(WorkflowOperation {
        id,
        identity: row.identity,
        kind,
        restart_from,
        target_generation,
        prior_state,
        created_at_ms,
        sequence,
    }))
}

pub(crate) fn verify_operations(conn: &rusqlite::Connection) -> Result<(), PlatformError> {
    let orphan: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_instance_referrers r
        WHERE r.state='restarting' AND NOT EXISTS(SELECT 1 FROM workflow_instance_operations o
          WHERE o.instance_id=r.instance_id AND o.kind='restart'))",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if orphan {
        return Err(invariant());
    }
    let mut statement = conn
        .prepare("SELECT instance_id FROM workflow_instance_operations")
        .map_err(sql_error)?;
    for id in statement
        .query_map([], |row| parse(row, 0))
        .map_err(sql_error)?
    {
        read_operation(conn, id.map_err(sql_error)?)?.ok_or_else(invariant)?;
    }
    Ok(())
}
