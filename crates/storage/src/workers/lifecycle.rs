use super::*;

impl<'a> WorkerRepository<'a> {
    /// Inspect aggregate deployment runtime admission without exposing tenant content.
    pub fn deployment_runtime_assessments(
        &self,
    ) -> Result<DeploymentRuntimeAssessmentSummary, PlatformError> {
        self.db.with_read(|connection| {
            let dispatchable: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM deployment_runtime_assessments WHERE state='dispatchable'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            let quarantined: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM deployment_runtime_assessments WHERE state='quarantined'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            let invalid_active: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM workers w
                     LEFT JOIN deployment_runtime_assessments a
                       ON a.deployment_id=w.active_deployment_id
                     WHERE w.active_deployment_id IS NOT NULL
                       AND (a.state IS NULL OR a.state!='dispatchable')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            let last_quarantine_reason = connection
                .query_row(
                    "SELECT reason FROM deployment_runtime_assessments
                     WHERE state='quarantined' ORDER BY updated_at_ms DESC, deployment_id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            Ok(DeploymentRuntimeAssessmentSummary {
                dispatchable: u64::try_from(dispatchable).map_err(|_| invariant())?,
                quarantined: u64::try_from(quarantined).map_err(|_| invariant())?,
                active_runtime_dispatchable: invalid_active == 0,
                last_quarantine_reason,
            })
        })
    }

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
    #[cfg(any(test, feature = "test-support"))]
    pub fn promote(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.promote_checked(
            instance_id,
            worker_id,
            target,
            expected_active,
            None,
            request_id,
            now_ms,
        )
    }

    /// Promote only if both the optional active pointer and route generation still match.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn promote_checked(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.get_tenant_worker(instance_id, worker_id)?;
        self.promote_worker_checked(
            instance_id,
            worker_id,
            target,
            expected_active,
            expected_route_generation,
            request_id,
            now_ms,
        )
    }

    /// Promote a version for any live Worker in the instance, including system-owned Workers.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn promote_worker_checked(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.create_deployment_checked(
            instance_id,
            worker_id,
            target,
            expected_active,
            expected_route_generation,
            if self.get_worker(instance_id, worker_id)?.ownership == WorkerOwnership::System {
                DeploymentSource::System
            } else {
                DeploymentSource::VersionsApi
            },
            &BTreeMap::new(),
            None,
            request_id,
            now_ms,
            open_compute_core::StartupId::generate(),
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
        instance_id: InstanceId,
        worker_id: WorkerId,
        target: VersionId,
        expected_active: Option<VersionId>,
        expected_route_generation: Option<u64>,
        source: DeploymentSource,
        annotations: &BTreeMap<String, String>,
        observability: Option<&WorkerObservabilityPatch>,
        request_id: RequestId,
        now_ms: i64,
        runtime_startup_id: open_compute_core::StartupId,
    ) -> Result<(WorkerRecord, DeploymentRecord), PlatformError> {
        if let Some(patch) = observability {
            validate_sampling_rate(patch.head_sampling_rate)?;
            validate_sampling_rate(patch.logs_head_sampling_rate)?;
        }
        let deployment_id = DeploymentId::generate();
        self.db.with_immediate(|tx| {
            let current = require_live_worker(tx, instance_id, worker_id)?;
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
            let quarantined: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_deployments d
                       JOIN deployment_runtime_assessments a ON a.deployment_id=d.id
                       WHERE d.worker_id=?1 AND d.version_id=?2 AND a.state='quarantined')",
                    params![worker_id.to_string(), target.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if quarantined {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "a quarantined Version cannot be activated again",
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
            tx.execute(
                "INSERT INTO deployment_runtime_assessments
                 (deployment_id, state, startup_id, reason, updated_at_ms)
                 VALUES (?1, 'dispatchable', ?2, NULL, ?3)",
                params![
                    deployment_id.to_string(),
                    runtime_startup_id.to_string(),
                    now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id = ?1,
                         route_generation = route_generation + 1, updated_at_ms = ?2
                     WHERE id = ?3 AND (SELECT instance_id FROM instance_identity) = ?4 AND deleted_at_ms IS NULL
                       AND route_generation = ?5",
                    params![
                        deployment_id.to_string(),
                        now_ms,
                        worker_id.to_string(),
                        instance_id.to_string(),
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
            if let Some(patch) = observability {
                let current = tx
                    .query_row(
                        "SELECT generation, enabled, head_sampling_rate, logs_enabled,
                                logs_head_sampling_rate, invocation_logs, persist, updated_at_ms
                         FROM worker_observability_settings WHERE worker_id = ?1",
                        [worker_id.to_string()],
                        map_observability_settings,
                    )
                    .optional()
                    .map_err(|_| db_error())?
                    .ok_or_else(invariant)?;
                let generation = current.generation.checked_add(1).ok_or_else(invariant)?;
                let updated = tx
                    .execute(
                        "UPDATE worker_observability_settings SET generation=?1, enabled=?2,
                           head_sampling_rate=?3, logs_enabled=?4, logs_head_sampling_rate=?5,
                           invocation_logs=?6, persist=?7, updated_at_ms=?8
                         WHERE worker_id=?9 AND generation=?10",
                        params![
                            i64::try_from(generation).map_err(|_| invariant())?,
                            patch.enabled.unwrap_or(current.enabled),
                            patch.head_sampling_rate.or(current.head_sampling_rate),
                            patch.logs_enabled.unwrap_or(current.logs_enabled),
                            patch
                                .logs_head_sampling_rate
                                .or(current.logs_head_sampling_rate),
                            patch.invocation_logs.unwrap_or(current.invocation_logs),
                            patch.persist.unwrap_or(current.persist),
                            now_ms,
                            worker_id.to_string(),
                            i64::try_from(current.generation).map_err(|_| invariant())?,
                        ],
                    )
                    .map_err(|_| db_error())?;
                if updated != 1 {
                    return Err(invariant());
                }
                audit(
                    tx,
                    "worker.observability.update",
                    "worker",
                    &worker_id.to_string(),
                    request_id,
                    format!(r#"{{"generation":{generation}}}"#).as_bytes(),
                    now_ms,
                )?;
            }
            audit(
                tx,
                "deployment.create",
                "deployment",
                &deployment_id.to_string(),
                request_id,
                br#"{"state":"active"}"#,
                now_ms,
            )?;
            Ok((
                read_worker_tx(tx, instance_id, worker_id)?,
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

    /// Quarantine one exactly attributed active deployment and atomically roll back.
    pub fn quarantine_active_deployment(
        &self,
        deployment_id: DeploymentId,
        reason: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        if reason.is_empty() || reason.len() > 128 || reason.chars().any(char::is_control) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let current: Option<(String, String)> = tx
                .query_row(
                    "SELECT w.id, (SELECT instance_id FROM instance_identity) FROM workers w
                     JOIN worker_deployments d ON d.id=w.active_deployment_id
                     JOIN deployment_runtime_assessments a ON a.deployment_id=d.id
                     WHERE d.id=?1 AND d.deleted_at_ms IS NULL AND a.state='dispatchable'",
                    [deployment_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((worker_id_text, account_id_text)) = current else {
                return Ok(false);
            };
            let previous = tx
                .query_row(
                    "SELECT d.id FROM worker_deployments d
                     JOIN deployment_runtime_assessments a ON a.deployment_id=d.id
                     JOIN worker_versions v ON v.id=d.version_id
                     WHERE d.worker_id=?1 AND d.id!=?2 AND d.deleted_at_ms IS NULL
                       AND a.state='dispatchable' AND v.state='ready'
                     ORDER BY d.created_at_ms DESC, d.id DESC LIMIT 1",
                    params![worker_id_text, deployment_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| db_error())?
                .map(|value| value.parse().map_err(|_| invariant()))
                .transpose()?;
            let updated = tx
                .execute(
                    "UPDATE deployment_runtime_assessments
                     SET state='quarantined', reason=?2, updated_at_ms=?3
                     WHERE deployment_id=?1 AND state='dispatchable'",
                    params![deployment_id.to_string(), reason, now_ms],
                )
                .map_err(|_| db_error())?;
            if updated != 1 {
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id=?1,
                       route_generation=route_generation+1, updated_at_ms=?2
                     WHERE id=?3 AND (SELECT instance_id FROM instance_identity)=?4 AND active_deployment_id=?5",
                    params![
                        previous.map(|value: DeploymentId| value.to_string()),
                        now_ms,
                        worker_id_text,
                        account_id_text,
                        deployment_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            audit(
                tx,
                "deployment.quarantine",
                "deployment",
                &deployment_id.to_string(),
                request_id,
                br#"{"state":"quarantined"}"#,
                now_ms,
            )?;
            Ok(true)
        })
    }
}
