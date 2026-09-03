use super::*;

impl WorkflowRepository<'_> {
    /// List a bounded immutable version page, ordered by monotonic version number.
    pub fn versions(
        &self,
        account: AccountId,
        definition: WorkflowId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<WorkflowVersion>, PlatformError> {
        if after < 0 || limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.definition(account, definition)?;
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(&format!("{VERSION_SELECT} WHERE f.account_id=?1 AND f.id=?2 AND v.version_number>?3 ORDER BY v.version_number LIMIT ?4")).map_err(sql_error)?;
            let versions = statement.query_map(params![account.to_string(),definition.to_string(),after,limit], version_row).map_err(sql_error)?.collect::<Result<Vec<_>,_>>().map_err(sql_error)?;
            for version in &versions {
                if version_digest(&version.target)? != version.target.descriptor_sha256 { return Err(invariant()); }
            }
            Ok(versions)
        })
    }

    /// Resume validation after a crash using a bounded, stable identity cursor.
    pub fn pending_versions(
        &self,
        after: Option<WorkflowVersionId>,
        limit: u32,
    ) -> Result<Vec<WorkflowVersion>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(&format!("{VERSION_SELECT} WHERE v.state='validating' AND (?1 IS NULL OR v.id>?1) ORDER BY v.id LIMIT ?2")).map_err(sql_error)?;
            let versions = statement.query_map(params![after.map(|id|id.to_string()),limit], version_row).map_err(sql_error)?.collect::<Result<Vec<_>,_>>().map_err(sql_error)?;
            for version in &versions {
                if version_digest(&version.target)? != version.target.descriptor_sha256 { return Err(invariant()); }
            }
            Ok(versions)
        })
    }

    /// Count active typed references without exposing their internal capability data.
    pub fn referrer_count(
        &self,
        account: AccountId,
        definition: WorkflowId,
    ) -> Result<u64, PlatformError> {
        self.definition(account, definition)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM workflow_referrers WHERE definition_id=?1",
                [definition.to_string()],
                |row| row.get(0),
            )
            .map_err(sql_error)
        })
    }

    /// Freeze a ready same-account version and protect it before asynchronous class validation.
    pub fn stage_version(
        &self,
        account: AccountId,
        definition: WorkflowId,
        version: VersionId,
        class_name: &str,
        now_ms: i64,
    ) -> Result<WorkflowVersion, PlatformError> {
        self.stage_version_inner(account, definition, version, class_name, None, now_ms)
    }

    /// Freeze a version only while the exact upload-before-PUT reservation still owns admission.
    pub fn stage_reserved_version(
        &self,
        account: AccountId,
        definition: WorkflowId,
        version: VersionId,
        class_name: &str,
        reservation: &WorkflowDefinitionReservation,
        now_ms: i64,
    ) -> Result<WorkflowVersion, PlatformError> {
        if reservation.definition.id != definition
            || reservation.definition.account_id != account
            || reservation.definition.reserved_class_name.as_deref() != Some(class_name)
        {
            return Err(invariant());
        }
        self.stage_version_inner(
            account,
            definition,
            version,
            class_name,
            Some((reservation.owner.as_str(), reservation.fence)),
            now_ms,
        )
    }

    fn stage_version_inner(
        self,
        account: AccountId,
        definition: WorkflowId,
        version: VersionId,
        class_name: &str,
        reservation: Option<(&str, i64)>,
        now_ms: i64,
    ) -> Result<WorkflowVersion, PlatformError> {
        validate_class_name(class_name)?;
        self.db.with_immediate(|tx| {
            let definition_row = tx.query_row(&format!("{DEFINITION_SELECT} WHERE id=?1 AND account_id=?2"),
                params![definition.to_string(),account.to_string()], definition_row).optional().map_err(sql_error)?
                .ok_or_else(||error(ErrorCode::WorkflowNotFound))?;
            if !matches!(definition_row.state, ResourceState::Creating|ResourceState::Ready) {
                return Err(error(ErrorCode::WorkflowNotReady));
            }
            match reservation {
                Some((owner, fence))
                    if definition_row.reserved_class_name.as_deref() == Some(class_name)
                        && definition_row.reservation_owner.as_deref() == Some(owner)
                        && definition_row.reservation_fence == fence
                        && definition_row.reservation_state.is_some() =>
                {
                    let changed = tx.execute(
                        "UPDATE workflow_definitions SET reservation_state='bound',updated_at_ms=?4
                         WHERE id=?1 AND reservation_owner=?2 AND reservation_fence=?3
                         AND reservation_state IN ('reserved','bound')",
                        params![definition.to_string(),owner,fence,now_ms],
                    ).map_err(sql_error)?;
                    if changed != 1 {
                        return Err(error(ErrorCode::WorkflowVersionNotReady));
                    }
                }
                None if definition_row.reserved_class_name.is_none() => {}
                _ => {
                    return Err(error(ErrorCode::WorkflowVersionNotReady));
                }
            }
            let version = tx.query_row("SELECT w.id,d.id,d.worker_code_sha256,d.loader_schema_version
                FROM worker_versions d JOIN workers w ON w.id=d.worker_id
                WHERE d.id=?1 AND w.account_id=?2 AND d.state='ready' AND w.deleted_at_ms IS NULL",
                params![version.to_string(),account.to_string()], |row| {
                    Ok((parse(row,0)?,parse(row,1)?,digest(row,2)?,row.get::<_,i64>(3)?))
                }).optional().map_err(sql_error)?.ok_or_else(||error(ErrorCode::WorkflowVersionNotReady))?;
            let version_number: i64 = tx.query_row("SELECT coalesce(MAX(version_number),0)+1 FROM workflow_versions WHERE definition_id=?1",
                [definition.to_string()],|row|row.get(0)).map_err(sql_error)?;
            if version_number > 10000 { return Err(error(ErrorCode::QuotaExceeded)); }
            let mut target = WorkflowTarget { account_id: account, definition_id: definition,
                definition_name: definition_row.name, workflow_version_id: WorkflowVersionId::generate(),
                worker_id: version.0, worker_version_id: version.1, worker_code_sha256: version.2,
                class_name: class_name.into(), loader_schema_version: version.3, capability_version: 1,
                descriptor_sha256: [0;32] };
            target.descriptor_sha256 = version_digest(&target)?;
            tx.execute("INSERT INTO workflow_versions(id,definition_id,version_number,state,worker_id,worker_version_id,
                class_name,reservation_owner,reservation_fence,worker_code_sha256,loader_schema_version,
                capability_version,descriptor_sha256,created_at_ms)
                VALUES(?1,?2,?3,'staging',?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)", params![target.workflow_version_id.to_string(),
                definition.to_string(),version_number,target.worker_id.to_string(),target.worker_version_id.to_string(),
                class_name,reservation.map(|value| value.0),reservation.map(|value| value.1),
                target.worker_code_sha256.as_slice(),target.loader_schema_version,1,target.descriptor_sha256.as_slice(),now_ms]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_versions SET state='validating' WHERE id=?1",[target.workflow_version_id.to_string()]).map_err(sql_error)?;
            Ok(WorkflowVersion { target, version_number, state: VersionState::Validating,
                created_at_ms: now_ms, rejection_code: None,
                reservation_owner: reservation.map(|value| value.0.to_owned()),
                reservation_fence: reservation.map(|value| value.1) })
        })
    }

    /// Read an immutable version and verify its canonical frozen descriptor.
    pub fn version(
        &self,
        account: AccountId,
        id: WorkflowVersionId,
    ) -> Result<WorkflowVersion, PlatformError> {
        let version = self.db.with_read(|conn| {
            conn.query_row(
                &format!("{VERSION_SELECT} WHERE f.account_id=?1 AND v.id=?2"),
                params![account.to_string(), id.to_string()],
                version_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowVersionNotReady))
        })?;
        if version_digest(&version.target)? != version.target.descriptor_sha256 {
            return Err(invariant());
        }
        Ok(version)
    }

    /// Commit a proven class, or retain a rejected version without disturbing an older current version.
    pub fn finish_version(
        &self,
        account: AccountId,
        id: WorkflowVersionId,
        accepted: bool,
        now_ms: i64,
    ) -> Result<WorkflowVersion, PlatformError> {
        self.db.with_immediate(|tx| {
            let version = tx.query_row(&format!("{VERSION_SELECT} WHERE f.account_id=?1 AND v.id=?2"),
                params![account.to_string(),id.to_string()],version_row).optional().map_err(sql_error)?
                .ok_or_else(||error(ErrorCode::WorkflowVersionNotReady))?;
            if version.state == if accepted { VersionState::Ready } else { VersionState::Rejected } { return Ok(version); }
            if version.state != VersionState::Validating { return Err(error(ErrorCode::WorkflowVersionNotReady)); }
            let definition = tx.query_row(
                &format!("{DEFINITION_SELECT} WHERE id=?1 AND account_id=?2"),
                params![version.target.definition_id.to_string(),account.to_string()],
                definition_row,
            ).map_err(sql_error)?;
            let reservation_is_current = match (&version.reservation_owner, version.reservation_fence) {
                (Some(owner), Some(fence)) => definition.reserved_class_name.as_deref()
                    == Some(version.target.class_name.as_str())
                    && definition.reservation_owner.as_deref() == Some(owner.as_str())
                    && definition.reservation_fence == fence,
                (None, None) => definition.reserved_class_name.is_none(),
                _ => false,
            };
            if accepted && reservation_is_current {
                tx.execute("UPDATE workflow_versions SET state='ready',ready_at_ms=?2 WHERE id=?1 AND state='validating'",
                    params![id.to_string(),now_ms]).map_err(sql_error)?;
                let changed = tx.execute("UPDATE workflow_definitions SET state='ready',availability='healthy',availability_code=NULL,
                    reserved_class_name=NULL,reservation_owner=NULL,reservation_state=NULL,reservation_created_definition=NULL,
                    current_version_id=?2,updated_at_ms=?3 WHERE id=?1 AND state IN ('creating','ready')
                    AND (current_version_id IS NULL OR (SELECT version_number FROM workflow_versions WHERE id=current_version_id)<?4)",
                    params![version.target.definition_id.to_string(),id.to_string(),now_ms,version.version_number]).map_err(sql_error)?;
                if changed != 1 && version.reservation_owner.is_some() {
                    return Err(invariant());
                }
            } else {
                tx.execute("UPDATE workflow_versions SET state='rejected',rejected_at_ms=?2,rejection_code='WORKFLOW_VERSION_NOT_READY'
                    WHERE id=?1 AND state='validating'",params![id.to_string(),now_ms]).map_err(sql_error)?;
                if let (Some(owner), Some(fence)) =
                    (&version.reservation_owner, version.reservation_fence)
                    && reservation_is_current
                {
                    let other_active = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM workflow_versions
                             WHERE definition_id=?1 AND reservation_owner=?2 AND reservation_fence=?3
                               AND id!=?4 AND state IN ('staging','validating','ready'))",
                            params![
                                version.target.definition_id.to_string(),
                                owner,
                                fence,
                                id.to_string()
                            ],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(sql_error)?;
                    if !other_active {
                        let cleared = tx
                            .execute(
                                "UPDATE workflow_definitions SET reserved_class_name=NULL,
                                 reservation_owner=NULL,reservation_state=NULL,
                                 reservation_created_definition=NULL,updated_at_ms=?5
                                 WHERE account_id=?1 AND id=?2 AND reservation_owner=?3
                                   AND reservation_fence=?4 AND state IN ('creating','ready')",
                                params![
                                    account.to_string(),
                                    version.target.definition_id.to_string(),
                                    owner,
                                    fence,
                                    now_ms
                                ],
                            )
                            .map_err(sql_error)?;
                        if cleared != 1 {
                            return Err(invariant());
                        }
                    }
                }
            }
            tx.query_row(&format!("{VERSION_SELECT} WHERE v.id=?1"),[id.to_string()],version_row).map_err(sql_error)
        })
    }

    /// Release bounded noncurrent versions only after every live instance has released its pin.
    pub fn retire_unused_versions(&self, limit: u32, now_ms: i64) -> Result<u64, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.db.with_immediate(|tx| {
            let mut statement = tx.prepare("SELECT v.id FROM workflow_versions v
                WHERE v.state IN ('ready','rejected') AND NOT EXISTS(SELECT 1 FROM workflow_definitions f WHERE f.current_version_id=v.id)
                AND NOT EXISTS(SELECT 1 FROM workflow_instance_referrers r WHERE r.workflow_version_id=v.id AND r.state!='released')
                ORDER BY v.created_at_ms,v.id LIMIT ?1").map_err(sql_error)?;
            let ids = statement.query_map([limit],|row|row.get::<_,String>(0)).map_err(sql_error)?
                .collect::<Result<Vec<_>,_>>().map_err(sql_error)?;
            for id in &ids {
                tx.execute("UPDATE workflow_versions SET state='deleting' WHERE id=?1",[id]).map_err(sql_error)?;
                tx.execute("UPDATE workflow_versions SET state='tombstoned',deleted_at_ms=?2 WHERE id=?1",params![id,now_ms]).map_err(sql_error)?;
            }
            Ok(ids.len() as u64)
        })
    }
}
