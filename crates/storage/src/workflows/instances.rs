use super::*;
use open_compute_core::WorkflowsConfig;

impl WorkflowRepository<'_> {
    /// Retain a proven terminal instance without releasing either immutable artifact pin.
    pub fn retain_instance(
        &self,
        identity: &WorkflowInstanceIdentity,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        version_digest(&identity.target)?;
        self.db.with_immediate(|tx| {
            verify_identity(tx, identity)?;
            let state: String = tx.query_row("SELECT state FROM workflow_instance_referrers WHERE instance_id=?1",
                [identity.instance_id.to_string()], |row| row.get(0)).map_err(sql_error)?;
            match state.as_str() {
                "retained" => Ok(()),
                "live" => {
                    tx.execute("UPDATE workflow_instance_referrers SET state='retained',updated_at_ms=?2 WHERE instance_id=?1",
                        params![identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
                    Ok(())
                }
                _ => Err(error(ErrorCode::WorkflowInstanceStateConflict)),
            }
        })
    }

    /// Reserve public identity and live artifact reachability before writing scheduler authority.
    pub fn reserve_instance(
        &self,
        account: AccountId,
        definition: WorkflowId,
        external: Option<&str>,
        limits: &WorkflowsConfig,
        now_ms: i64,
    ) -> Result<WorkflowReservation, PlatformError> {
        let external =
            external.map_or_else(|| WorkflowInstanceId::generate().to_string(), str::to_owned);
        open_compute_core::workflow::validate_workflow_instance_id(&external)?;
        limits.validate()?;
        self.db.with_immediate(|tx| {
            let version = tx.query_row(&format!("{VERSION_SELECT} WHERE f.account_id=?1 AND f.id=?2
                AND f.state='ready' AND f.availability='healthy' AND f.current_version_id=v.id AND v.state='ready'"),
                params![account.to_string(),definition.to_string()],version_row).optional().map_err(sql_error)?
                .ok_or_else(||error(ErrorCode::WorkflowNotReady))?;
            if version_digest(&version.target)? != version.target.descriptor_sha256 { return Err(invariant()); }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_instance_referrers WHERE definition_id=?1 AND external_instance_id=?2)",
                params![definition.to_string(),external],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowInstanceAlreadyExists));
            }
            let (total,active,definition_total): (u64,u64,u64) = tx.query_row(
                "SELECT COUNT(*),coalesce(SUM(r.state IN ('creating','live','restarting')),0),coalesce(SUM(r.definition_id=?2),0)
                 FROM workflow_instance_referrers r JOIN workflow_definitions f ON f.id=r.definition_id WHERE f.account_id=?1",
                params![account.to_string(),definition.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(sql_error)?;
            if total >= u64::from(limits.max_instances_per_account) || active >= u64::from(limits.max_active_per_account)
                || definition_total >= u64::from(limits.max_instances_per_definition) { return Err(error(ErrorCode::WorkflowStateQuotaExceeded)); }
            let identity = WorkflowInstanceIdentity { instance_id: WorkflowInstanceId::generate(),external_instance_id: external,
                target: version.target,instance_generation: 1,creation_nonce: token()?,created_at_ms: now_ms };
            tx.execute("INSERT INTO workflow_instance_referrers(instance_id,definition_id,definition_name,external_instance_id,
                version_id,deployment_id,instance_generation,creation_nonce,state,created_at_ms,updated_at_ms)
                VALUES(?1,?2,?3,?4,?5,?6,1,?7,'creating',?8,?8)", params![identity.instance_id.to_string(),definition.to_string(),
                identity.target.definition_name,identity.external_instance_id,identity.target.version_id.to_string(),
                identity.target.deployment_id.to_string(),identity.creation_nonce.as_bytes().as_slice(),now_ms]).map_err(sql_error)?;
            Ok(WorkflowReservation { identity,state: WorkflowRefState::Creating,updated_at_ms: now_ms })
        })
    }

    /// Read one control reservation by its internal, non-tenant identity.
    pub fn reservation(
        &self,
        id: WorkflowInstanceId,
    ) -> Result<Option<WorkflowReservation>, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                &format!("{RESERVATION_SELECT} WHERE r.instance_id=?1"),
                [id.to_string()],
                reservation_row,
            )
            .optional()
            .map_err(sql_error)
        })
    }

    /// Resolve public instance identity in the binding's definition, never globally.
    pub fn find_instance(
        &self,
        definition: WorkflowId,
        external: &str,
    ) -> Result<WorkflowReservation, PlatformError> {
        open_compute_core::workflow::validate_workflow_instance_id(external)?;
        self.db.with_read(|conn| {
            conn.query_row(
                &format!(
                    "{RESERVATION_SELECT} WHERE r.definition_id=?1 AND r.external_instance_id=?2"
                ),
                params![definition.to_string(), external],
                reservation_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))
        })
    }

    /// Finalize only the exact control reservation after scheduler insert is durably committed.
    pub fn finalize_instance(
        &self,
        reservation: &WorkflowInstanceIdentity,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            verify_identity(tx,reservation)?;
            let changed = tx.execute("UPDATE workflow_instance_referrers SET state='live',updated_at_ms=?3
                WHERE instance_id=?1 AND creation_nonce=?2 AND state='creating'",params![reservation.instance_id.to_string(),
                reservation.creation_nonce.as_bytes().as_slice(),now_ms]).map_err(sql_error)?;
            if changed == 0 && !tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_instance_referrers WHERE instance_id=?1 AND creation_nonce=?2)",
                params![reservation.instance_id.to_string(),reservation.creation_nonce.as_bytes().as_slice()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(invariant());
            }
            Ok(())
        })
    }

    /// Release a proven-uncommitted creation; caller must first prove scheduler absence.
    pub fn abandon_creation(
        &self,
        reservation: &WorkflowInstanceIdentity,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "DELETE FROM workflow_instance_referrers WHERE instance_id=?1
            AND creation_nonce=?2 AND state='creating'",
                params![
                    reservation.instance_id.to_string(),
                    reservation.creation_nonce.as_bytes().as_slice()
                ],
            )
            .map(|count| count == 1)
            .map_err(sql_error)
        })
    }

    /// Bounded live-reservation reconciliation page, including healthy instances for release checks.
    pub fn live_reservations(
        &self,
        after: Option<WorkflowInstanceId>,
        limit: u32,
    ) -> Result<Vec<WorkflowReservation>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "{RESERVATION_SELECT} WHERE r.state!='released' AND r.instance_id>?1
                ORDER BY r.instance_id LIMIT ?2"
                ))
                .map_err(sql_error)?;
            statement
                .query_map(
                    params![after.map_or_else(String::new, |id| id.to_string()), limit],
                    reservation_row,
                )
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)
        })
    }

    /// Verify typed referrers without repairing or guessing immutable target authority.
    pub fn instance_referrers_intact(
        &self,
        reservation: &WorkflowInstanceIdentity,
    ) -> Result<bool, PlatformError> {
        self.db
            .with_read(|conn| referrers_intact(conn, reservation))
    }

    /// Rebuild missing typed references only from an exact, still-live control reservation.
    pub fn repair_instance_referrers(
        &self,
        identity: &WorkflowInstanceIdentity,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let reservation = tx.query_row(&format!("{RESERVATION_SELECT} WHERE r.instance_id=?1"),
                [identity.instance_id.to_string()],reservation_row).optional().map_err(sql_error)?.ok_or_else(invariant)?;
            if reservation.identity != *identity || !matches!(reservation.state,WorkflowRefState::Creating|WorkflowRefState::Live) { return Err(invariant()); }
            tx.execute("INSERT OR IGNORE INTO deployment_referrers(deployment_id,kind,ref_id,created_at_ms) VALUES(?1,'workflow_instance',?2,?3)",
                params![identity.target.deployment_id.to_string(),identity.instance_id.to_string(),identity.created_at_ms]).map_err(sql_error)?;
            tx.execute("INSERT OR IGNORE INTO workflow_referrers(definition_id,referrer_kind,referrer_id,created_at_ms) VALUES(?1,'instance',?2,?3)",
                params![identity.target.definition_id.to_string(),identity.instance_id.to_string(),identity.created_at_ms]).map_err(sql_error)?;
            Ok(())
        })
    }
}

pub(super) const RESERVATION_SELECT: &str = "SELECT f.account_id,r.definition_id,r.definition_name,r.version_id,v.worker_id,r.deployment_id,
    v.worker_code_sha256,v.class_name,v.loader_schema_version,v.capability_version,v.descriptor_sha256,
    r.instance_id,r.external_instance_id,r.instance_generation,r.creation_nonce,r.state,r.created_at_ms,r.updated_at_ms
    FROM workflow_instance_referrers r JOIN workflow_versions v ON v.id=r.version_id
    JOIN workflow_definitions f ON f.id=r.definition_id";

pub(super) fn verify_identity(
    conn: &rusqlite::Connection,
    identity: &WorkflowInstanceIdentity,
) -> Result<(), PlatformError> {
    let stored = conn
        .query_row(
            &format!("{RESERVATION_SELECT} WHERE r.instance_id=?1"),
            [identity.instance_id.to_string()],
            reservation_row,
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(invariant)?;
    if stored.identity != *identity
        || version_digest(&identity.target)? != identity.target.descriptor_sha256
    {
        return Err(invariant());
    }
    Ok(())
}

pub(super) fn referrers_intact(
    conn: &rusqlite::Connection,
    identity: &WorkflowInstanceIdentity,
) -> Result<bool, PlatformError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM deployment_referrers
        WHERE deployment_id=?1 AND kind='workflow_instance' AND ref_id=?2 AND created_at_ms=?4)
        AND EXISTS(SELECT 1 FROM workflow_referrers WHERE definition_id=?3 AND referrer_kind='instance' AND referrer_id=?2 AND created_at_ms=?4)",
        params![identity.target.deployment_id.to_string(),identity.instance_id.to_string(),identity.target.definition_id.to_string(),identity.created_at_ms],
        |row|row.get(0)).map_err(sql_error)
}

pub(super) fn reservation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowReservation> {
    let state = match row.get::<_, String>(15)?.as_str() {
        "creating" => WorkflowRefState::Creating,
        "live" => WorkflowRefState::Live,
        "retained" => WorkflowRefState::Retained,
        "restarting" => WorkflowRefState::Restarting,
        "releasing" => WorkflowRefState::Releasing,
        "released" => WorkflowRefState::Released,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(WorkflowReservation {
        identity: WorkflowInstanceIdentity {
            target: target_row(row)?,
            instance_id: parse(row, 11)?,
            external_instance_id: row.get(12)?,
            instance_generation: row.get(13)?,
            creation_nonce: WorkflowToken::from_bytes(digest(row, 14)?),
            created_at_ms: row.get(16)?,
        },
        state,
        updated_at_ms: row.get(17)?,
    })
}
