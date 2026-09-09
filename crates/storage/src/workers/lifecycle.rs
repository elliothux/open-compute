use super::*;

impl<'a> WorkerRepository<'a> {
    /// Transition staging to validating.
    pub fn begin_validation(&self, version_id: VersionId) -> Result<(), PlatformError> {
        self.transition(
            version_id,
            VersionState::Staging,
            VersionState::Validating,
            0,
            None,
        )
    }

    /// Mark a validating version ready.
    pub fn mark_ready(&self, version_id: VersionId, now_ms: i64) -> Result<(), PlatformError> {
        self.transition(
            version_id,
            VersionState::Validating,
            VersionState::Ready,
            now_ms,
            None,
        )
    }

    /// Atomically publish a validated Version and its prepared Durable Object migration.
    pub fn mark_ready_with_durable_object_migration(
        &self,
        version_id: VersionId,
        worker_id: WorkerId,
        plan: &crate::DurableObjectMigrationPlan,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE worker_versions SET state = 'ready', ready_at_ms = ?1
                     WHERE id = ?2 AND worker_id = ?3 AND state = 'validating'",
                    params![now_ms, version_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "version state transition precondition failed",
                ));
            }
            crate::durable_objects::publish_worker_migration_tx(
                tx, worker_id, version_id, plan, now_ms,
            )
        })
    }

    /// Reject a staging or validating version with a stable safe code.
    pub fn mark_rejected(
        &self,
        version_id: VersionId,
        expected: VersionState,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !matches!(expected, VersionState::Staging | VersionState::Validating) {
            return Err(invariant());
        }
        self.transition(
            version_id,
            expected,
            VersionState::Rejected,
            now_ms,
            Some(code.as_str()),
        )
    }

    fn transition(
        self,
        version_id: VersionId,
        expected: VersionState,
        target: VersionState,
        now_ms: i64,
        rejection_code: Option<&str>,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE worker_versions
                 SET state = ?1,
                     ready_at_ms = CASE WHEN ?1 = 'ready' THEN ?2 ELSE ready_at_ms END,
                     rejected_at_ms = CASE WHEN ?1 = 'rejected' THEN ?2 ELSE rejected_at_ms END,
                     rejection_code = CASE WHEN ?1 = 'rejected' THEN ?3 ELSE rejection_code END
                 WHERE id = ?4 AND state = ?5",
                    params![
                        target.as_str(),
                        now_ms,
                        rejection_code,
                        version_id.to_string(),
                        expected.as_str()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "version state transition precondition failed",
                ));
            }
            Ok(())
        })
    }

    /// Atomically promote a ready version, optionally using compare-and-swap.
    pub fn promote(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.promote_checked(
            account_id,
            worker_id,
            target,
            expected_active,
            None,
            request_id,
            now_ms,
        )
    }

    /// Promote only if both the optional active pointer and route generation still match.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn promote_checked(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.promote_worker_checked(
            account_id,
            worker_id,
            target,
            expected_active,
            expected_route_generation,
            request_id,
            now_ms,
        )
    }

    /// Promote a version for any live Worker in the account, including system-owned Workers.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn promote_worker_checked(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.create_deployment_checked(
            account_id,
            worker_id,
            target,
            expected_active,
            expected_route_generation,
            if self.get_worker(account_id, worker_id)?.ownership == WorkerOwnership::System {
                DeploymentSource::System
            } else {
                DeploymentSource::VersionsApi
            },
            &BTreeMap::new(),
            request_id,
            now_ms,
        )
        .map(|(worker, _)| worker)
    }

    /// Create one immutable 100-percent Deployment and atomically make it current.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn create_deployment_checked(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        source: DeploymentSource,
        annotations: &BTreeMap<String, String>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(WorkerRecord, DeploymentRecord), PlatformError> {
        let deployment_id = DeploymentId::generate();
        self.db.with_immediate(|tx| {
            let current = require_live_worker(tx, account_id, worker_id)?;
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_versions WHERE id = ?1 AND worker_id = ?2",
                    params![target.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some(state) = state else {
                return Err(version_not_found());
            };
            if state != "ready" {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "Deployment target is not a ready Version of this Worker",
                ));
            }
            if expected_active.is_some_and(|expected| current.active_version_id != Some(expected))
                || expected_route_generation
                    .is_some_and(|expected| current.route_generation != expected)
            {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Deployment compare-and-swap precondition failed",
                ));
            }
            tx.execute(
                "INSERT INTO worker_deployments
                 (id, worker_id, version_id, source, annotations_json, created_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![
                    deployment_id.to_string(),
                    worker_id.to_string(),
                    target.to_string(),
                    source.as_str(),
                    serde_json::to_vec(annotations).map_err(|_| invariant())?,
                    now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id = ?1,
                         route_generation = route_generation + 1, updated_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND deleted_at_ms IS NULL
                       AND route_generation = ?5",
                    params![
                        deployment_id.to_string(),
                        now_ms,
                        worker_id.to_string(),
                        account_id.to_string(),
                        i64::try_from(current.route_generation).map_err(|_| invariant())?
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Deployment compare-and-swap precondition failed",
                ));
            }
            audit(
                tx,
                account_id,
                "deployment.create",
                "deployment",
                &deployment_id.to_string(),
                request_id,
                br#"{"state":"active"}"#,
                now_ms,
            )?;
            Ok((
                read_worker_tx(tx, account_id, worker_id)?,
                DeploymentRecord {
                    id: deployment_id,
                    worker_id,
                    version_id: target,
                    source,
                    annotations: annotations.clone(),
                    created_at_ms: now_ms,
                    deleted_at_ms: None,
                },
            ))
        })
    }
}
