use super::*;

impl<'a> WorkerRepository<'a> {
    /// List force deletions that must complete before runtime admission starts.
    pub fn force_delete_intents(&self) -> Result<Vec<WorkerDeleteIntent>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT account_id, worker_id, request_id
                     FROM worker_delete_intents ORDER BY created_at_ms, worker_id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|_| db_error())?;
            let mut intents = Vec::new();
            for row in rows {
                let (account_id, worker_id, request_id) = row.map_err(|_| db_error())?;
                intents.push(WorkerDeleteIntent {
                    account_id: AccountId::from_str(&account_id).map_err(|_| invariant())?,
                    worker_id: WorkerId::from_str(&worker_id).map_err(|_| invariant())?,
                    request_id: RequestId::from_str(&request_id).map_err(|_| invariant())?,
                });
            }
            Ok(intents)
        })
    }

    /// Persist a crash-recoverable force-delete fence before runtime rotation.
    pub fn begin_force_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
            tx.execute(
                "INSERT INTO worker_delete_intents(worker_id, account_id, request_id, created_at_ms)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(worker_id) DO NOTHING",
                params![
                    worker_id.to_string(),
                    account_id.to_string(),
                    request_id.to_string(),
                    now_ms
                ],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Read an immutable version with vars and secret ciphertext in one snapshot.
    pub fn version_snapshot(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        allow_validating: bool,
    ) -> Result<VersionSnapshot, PlatformError> {
        self.db.with_read(|conn| {
            let worker = conn
                .query_row(
                    "SELECT id, account_id, name,
                        (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                        do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                        ownership, active_deployment_id
                 FROM workers WHERE id = ?1 AND account_id = ?2",
                    params![worker_id.to_string(), account_id.to_string()],
                    map_worker,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(worker_not_found)?;
            if worker.deleted_at_ms.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::WorkerDeleted,
                    "Worker is tombstoned",
                ));
            }
            let version = conn
                .query_row(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms,
                        compatibility_date, compatibility_flags_json,
                        resource_limits_json
                 FROM worker_versions WHERE id = ?1 AND worker_id = ?2",
                    params![version_id.to_string(), worker_id.to_string()],
                    map_version,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(version_not_found)?;
            if version.state != VersionState::Ready
                && !(allow_validating && version.state == VersionState::Validating)
            {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "version is not available to RuntimeSource",
                ));
            }
            let vars = read_vars(conn, version_id)?;
            let secrets = read_secrets(conn, version_id)?;
            let bindings = crate::bindings::read_version_bindings_conn(conn, version_id)?;
            let queue_bindings = crate::queues::read_version_bindings_conn(conn, version_id)?;
            Ok(VersionSnapshot {
                account_id,
                worker,
                assets: crate::assets::read_assets_conn(conn, version_id)?,
                version,
                annotations: read_version_annotations(conn, version_id)?,
                vars,
                secrets,
                bindings,
                queue_bindings,
                workflow_bindings: crate::workflows::bindings::read_workflow_bindings(
                    conn, version_id,
                )?,
                services: crate::services::read_version_services_conn(conn, version_id)?,
                cache_policies: crate::runtime_features::read_cache_policies_conn(
                    conn, version_id,
                )?,
                builtin_bindings: crate::runtime_features::read_builtin_bindings_conn(
                    conn, version_id,
                )?,
            })
        })
    }

    /// List all versions, newest first.
    pub fn list_versions(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<VersionRecord>, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms,
                        compatibility_date, compatibility_flags_json,
                        resource_limits_json
                 FROM worker_versions WHERE worker_id = ?1
                 ORDER BY version_number DESC",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([worker_id.to_string()], map_version)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Read immutable closed Cloudflare annotations for one account-scoped Version.
    pub fn version_annotations(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<BTreeMap<String, String>, PlatformError> {
        self.get_worker_version(account_id, worker_id, version_id)?;
        self.db
            .with_read(|conn| read_version_annotations(conn, version_id))
    }

    /// Read one version while enforcing the account and Worker boundary.
    pub fn get_version(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<VersionRecord, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.get_worker_version(account_id, worker_id, version_id)
    }

    /// Authorize a ready tenant version whose owning Worker is still live.
    pub fn authorize_runtime_version(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<VersionRecord, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT d.id, d.worker_id, d.version_number, d.content_kind, d.state,
                        d.artifact_sha256, d.artifact_size, d.artifact_schema_version,
                        d.main_module, d.worker_code_sha256, d.loader_schema_version,
                        d.created_at_ms, d.ready_at_ms, d.rejected_at_ms, d.rejection_code,
                        d.deleted_at_ms, d.compatibility_date, d.compatibility_flags_json,
                        d.resource_limits_json
                 FROM worker_versions d
                 JOIN workers w ON w.id = d.worker_id
                 WHERE d.id = ?1 AND d.worker_id = ?2 AND w.account_id = ?3
                   AND w.ownership = 'tenant' AND w.deleted_at_ms IS NULL
                   AND d.state = 'ready' AND d.deleted_at_ms IS NULL",
                params![
                    version_id.to_string(),
                    worker_id.to_string(),
                    account_id.to_string()
                ],
                map_version,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(version_not_found)
        })
    }

    /// Read one version for any Worker in the account, including system-owned Workers.
    pub fn get_worker_version(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<VersionRecord, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms,
                        compatibility_date, compatibility_flags_json,
                        resource_limits_json
                 FROM worker_versions WHERE id = ?1 AND worker_id = ?2",
                params![version_id.to_string(), worker_id.to_string()],
                map_version,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(version_not_found)
        })
    }

    /// List immutable Deployment history newest first.
    pub fn list_deployments(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<DeploymentRecord>, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id,worker_id,version_id,source,annotations_json,created_at_ms,deleted_at_ms
                     FROM worker_deployments WHERE worker_id=?1 AND deleted_at_ms IS NULL
                     ORDER BY created_at_ms DESC,id DESC",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([worker_id.to_string()], map_deployment)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Read one immutable Deployment.
    pub fn get_deployment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
    ) -> Result<DeploymentRecord, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.get_worker_deployment(account_id, worker_id, deployment_id)
    }

    /// Read one immutable Deployment for any live Worker, including system-owned Workers.
    pub fn get_worker_deployment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
    ) -> Result<DeploymentRecord, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT id,worker_id,version_id,source,annotations_json,created_at_ms,deleted_at_ms
                 FROM worker_deployments WHERE id=?1 AND worker_id=?2 AND deleted_at_ms IS NULL",
                params![deployment_id.to_string(), worker_id.to_string()],
                map_deployment,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(version_not_found)
        })
    }

    /// Tombstone a non-current Deployment without changing Version history.
    pub fn delete_deployment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE worker_deployments SET deleted_at_ms=?1
                     WHERE id=?2 AND worker_id=?3 AND deleted_at_ms IS NULL
                       AND NOT EXISTS(SELECT 1 FROM workers WHERE active_deployment_id=?2)",
                    params![now_ms, deployment_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::VersionActive,
                    "Deployment is current, missing, or already deleted",
                ));
            }
            audit(
                tx,
                account_id,
                "deployment.delete",
                "deployment",
                &deployment_id.to_string(),
                request_id,
                br#"{"state":"deleted"}"#,
                now_ms,
            )
        })
    }

    /// Resolve one active canonical hostname on a trusted listener and freeze its active version.
    pub fn resolve_route(
        &self,
        hostname_ascii: &str,
        path: &str,
        exposure: WorkerOriginExposure,
    ) -> Result<RouteSnapshot, PlatformError> {
        self.db.with_read(|conn| {
            let route = conn
                .query_row(
                    "SELECT r.id, r.account_id, r.worker_id, c.hostname_ascii,
                            r.path_prefix, r.entrypoint, r.generation, r.created_at_ms,
                            r.exposure
                     FROM hostname_claims c
                     JOIN worker_host_routes r ON r.claim_id = c.id
                     WHERE c.hostname_ascii = ?1
                       AND c.namespace = 'worker' AND c.exposure = ?3
                       AND r.namespace = c.namespace AND r.exposure = c.exposure
                       AND c.state = 'active' AND r.state = 'active'
                       AND ?2 LIKE r.path_prefix || '%'
                     LIMIT 1",
                    params![hostname_ascii, path, exposure.as_str()],
                    map_route,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            let worker = conn
                .query_row(
                    "SELECT id, account_id, name,
                        (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                        do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                        ownership, active_deployment_id
                 FROM workers WHERE id = ?1 AND account_id = ?2 AND deleted_at_ms IS NULL",
                    params![route.worker_id.to_string(), route.account_id.to_string()],
                    map_worker,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            let active_deployment = worker.active_deployment_id.ok_or_else(route_not_found)?;
            let deployment = conn
                .query_row(
                    "SELECT id,worker_id,version_id,source,annotations_json,created_at_ms,deleted_at_ms
                     FROM worker_deployments WHERE id=?1 AND worker_id=?2 AND deleted_at_ms IS NULL",
                    params![active_deployment.to_string(), worker.id.to_string()],
                    map_deployment,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            let active = deployment.version_id;
            let version = conn
                .query_row(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms,
                        compatibility_date, compatibility_flags_json,
                        resource_limits_json
                 FROM worker_versions WHERE id = ?1 AND worker_id = ?2 AND state = 'ready'",
                    params![active.to_string(), worker.id.to_string()],
                    map_version,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            Ok(RouteSnapshot {
                route,
                worker,
                deployment,
                assets: crate::assets::read_assets_conn(conn, active)?,
                version,
            })
        })
    }

    /// List active routes owned by one live Worker visible to tenant APIs.
    pub fn list_routes(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<RouteRecord>, PlatformError> {
        self.get_tenant_worker(account_id, worker_id)?;
        self.list_worker_routes(account_id, worker_id)
    }

    /// List active routes for any live Worker in the account, including system-owned Workers.
    pub fn list_worker_routes(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<RouteRecord>, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT r.id, r.account_id, r.worker_id, c.hostname_ascii,
                            r.path_prefix, r.entrypoint, r.generation, r.created_at_ms,
                            r.exposure
                     FROM worker_host_routes r
                     JOIN hostname_claims c ON c.id = r.claim_id
                     WHERE r.account_id = ?1 AND r.worker_id = ?2
                       AND r.state = 'active' AND c.state = 'active'
                     ORDER BY c.hostname_ascii, r.id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map(
                    params![account_id.to_string(), worker_id.to_string()],
                    map_route,
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Atomically verify a pre-fenced version set, disable routes, and tombstone a Worker.
    pub fn delete_worker(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        expected_versions: &[VersionId],
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.delete_worker_inner(
            account_id,
            worker_id,
            expected_versions,
            request_id,
            now_ms,
            false,
        )
    }

    /// Complete a previously admitted force delete, including inbound Service references.
    pub fn finish_force_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        expected_versions: &[VersionId],
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.delete_worker_inner(
            account_id,
            worker_id,
            expected_versions,
            request_id,
            now_ms,
            true,
        )
    }

    fn delete_worker_inner(
        self,
        account_id: AccountId,
        worker_id: WorkerId,
        expected_versions: &[VersionId],
        request_id: RequestId,
        now_ms: i64,
        force: bool,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
            let intent: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_delete_intents
                                   WHERE worker_id=?1 AND account_id=?2)",
                    params![worker_id.to_string(), account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if force != intent {
                return Err(PlatformError::new(
                    ErrorCode::VersionReferenced,
                    if force {
                        "force deletion intent is missing"
                    } else {
                        "force deletion is already in progress"
                    },
                ));
            }
            let actual_versions = {
                let mut statement = tx
                    .prepare(
                        "SELECT id FROM worker_versions
                         WHERE worker_id = ?1 AND deleted_at_ms IS NULL ORDER BY id",
                    )
                    .map_err(|_| db_error())?;
                let rows = statement
                    .query_map([worker_id.to_string()], |row| row.get::<_, String>(0))
                    .map_err(|_| db_error())?;
                let mut values = Vec::new();
                for row in rows {
                    values.push(
                        VersionId::from_str(&row.map_err(|_| db_error())?)
                            .map_err(|_| invariant())?,
                    );
                }
                values
            };
            let mut expected = expected_versions.to_vec();
            expected.sort_unstable_by_key(ToString::to_string);
            expected.dedup();
            if actual_versions != expected {
                return Err(PlatformError::new(
                    ErrorCode::VersionReferenced,
                    "Worker version set changed during deletion admission",
                ));
            }
            let inbound: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM version_services s
                        JOIN worker_versions d ON d.id = s.version_id
                        JOIN workers caller ON caller.id = d.worker_id
                        WHERE s.target_kind = 'worker' AND s.target_worker_id = ?1
                          AND caller.id != ?1
                          AND caller.account_id = ?2
                          AND caller.deleted_at_ms IS NULL
                          AND d.state IN ('staging', 'validating', 'ready')
                    )",
                    params![worker_id.to_string(), account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if inbound && !force {
                return Err(PlatformError::new(
                    ErrorCode::ServiceTargetReferenced,
                    "Worker is retained by another version Service declaration",
                ));
            }
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            tx.execute(
                "UPDATE worker_host_routes SET state = 'tombstoned', generation = ?1,
                        updated_at_ms = ?2, deleted_at_ms = ?2
                 WHERE account_id = ?3 AND worker_id = ?4 AND state = 'active'",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    account_id.to_string(),
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "UPDATE hostname_claims SET state = 'tombstoned', generation = ?1,
                        updated_at_ms = ?2, deleted_at_ms = ?2
                 WHERE id IN (SELECT claim_id FROM worker_host_routes
                              WHERE account_id = ?3 AND worker_id = ?4)
                   AND state = 'active'",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    account_id.to_string(),
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id = NULL,
                            route_generation = ?1, updated_at_ms = ?2, deleted_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND deleted_at_ms IS NULL",
                    params![
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms,
                        worker_id.to_string(),
                        account_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(worker_not_found());
            }
            tx.execute(
                "DELETE FROM resource_referrers
                 WHERE referrer_kind = 'version_binding'
                   AND referrer_id IN (
                     SELECT b.id FROM version_bindings b
                     JOIN worker_versions d ON d.id = b.version_id
                     WHERE d.worker_id = ?1
                   )",
                [worker_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM queue_referrers
                 WHERE referrer_kind = 'producer_binding'
                   AND referrer_id IN (
                     SELECT b.id FROM queue_producer_bindings b
                     JOIN worker_versions d ON d.id = b.version_id
                     WHERE d.worker_id = ?1
                   )",
                [worker_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM workflow_referrers
                 WHERE referrer_kind = 'binding'
                   AND referrer_id IN (
                     SELECT b.id FROM workflow_bindings b
                     JOIN worker_versions d ON d.id = b.version_id
                     WHERE d.worker_id = ?1
                   )",
                [worker_id.to_string()],
            )
            .map_err(|_| db_error())?;
            if force {
                tx.execute(
                    "DELETE FROM worker_delete_intents WHERE worker_id=?1 AND account_id=?2",
                    params![worker_id.to_string(), account_id.to_string()],
                )
                .map_err(|_| db_error())?;
            }
            audit(
                tx,
                account_id,
                "worker.delete",
                "worker",
                &worker_id.to_string(),
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )
        })
    }
}
