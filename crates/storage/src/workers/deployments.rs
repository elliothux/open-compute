use super::*;

impl<'a> WorkerRepository<'a> {
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
                        compatibility_date, compatibility_flags_json
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
                        compatibility_date, compatibility_flags_json
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
                        compatibility_date, compatibility_flags_json
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

    /// Resolve the longest active exact-host or platform path route and freeze active version.
    pub fn resolve_route(
        &self,
        hostname_ascii: Option<&str>,
        path: &str,
    ) -> Result<RouteSnapshot, PlatformError> {
        self.db.with_read(|conn| {
            let sql = if hostname_ascii.is_some() {
                "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                        entrypoint, generation
                 FROM worker_routes
                 WHERE kind = 'exact_host' AND hostname_ascii = ?1 AND state = 'active'
                   AND ?2 LIKE path_prefix || '%'
                 ORDER BY length(path_prefix) DESC LIMIT 1"
            } else {
                "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                        entrypoint, generation
                 FROM worker_routes
                 WHERE kind = 'platform_path' AND state = 'active'
                   AND ?2 LIKE path_prefix || '%'
                 ORDER BY length(path_prefix) DESC LIMIT 1"
            };
            let route = conn
                .query_row(sql, params![hostname_ascii.unwrap_or(""), path], map_route)
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
                        compatibility_date, compatibility_flags_json
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

    /// Add an exact-host route while atomically enforcing the account route limit.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn create_exact_route(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        hostname_ascii: &str,
        path_prefix: &str,
        entrypoint: Option<&str>,
        expected_active: Option<VersionId>,
        request_id: RequestId,
        now_ms: i64,
        max_live: u32,
    ) -> Result<RouteRecord, PlatformError> {
        validate_exact_route(hostname_ascii, path_prefix, entrypoint)?;
        if max_live == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "route count limit must be greater than zero",
            ));
        }
        let route_id = Uuid::now_v7().to_string();
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM worker_routes
                     WHERE account_id = ?1 AND state = 'active' AND deleted_at_ms IS NULL",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if live_count >= i64::from(max_live) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "account route count quota was exceeded",
                ));
            }
            if expected_active.is_some_and(|expected| worker.active_version_id != Some(expected)) {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "route entrypoint probe snapshot changed",
                ));
            }
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO worker_routes
                 (id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                  entrypoint, state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, 'exact_host', ?4, ?5, ?6,
                         'active', ?7, ?8, ?8, NULL)",
                    params![
                        route_id,
                        account_id.to_string(),
                        worker_id.to_string(),
                        hostname_ascii,
                        path_prefix,
                        entrypoint,
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
            if inserted != 1 {
                return Err(PlatformError::new(
                    ErrorCode::RouteConflict,
                    "an active route already owns this host and path prefix",
                ));
            }
            tx.execute(
                "UPDATE workers SET route_generation = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "route.create",
                "route",
                &route_id,
                request_id,
                br#"{"state":"active"}"#,
                now_ms,
            )?;
            Ok(RouteRecord {
                id: route_id.clone(),
                account_id,
                worker_id,
                kind: RouteKind::ExactHost,
                hostname_ascii: Some(hostname_ascii.to_owned()),
                path_prefix: path_prefix.to_owned(),
                entrypoint: entrypoint.map(ToOwned::to_owned),
                generation,
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
                    "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                            entrypoint, generation
                     FROM worker_routes
                     WHERE account_id = ?1 AND worker_id = ?2 AND state = 'active'
                     ORDER BY kind, hostname_ascii, path_prefix, id",
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

    /// Tombstone one exact-host route and increment the Worker route generation.
    pub fn delete_route(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        route_id: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            let changed = tx
                .execute(
                    "UPDATE worker_routes
                     SET state = 'tombstoned', generation = ?1, updated_at_ms = ?2,
                         deleted_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND worker_id = ?5
                       AND kind = 'exact_host' AND state = 'active'",
                    params![
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms,
                        route_id,
                        account_id.to_string(),
                        worker_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(route_not_found());
            }
            tx.execute(
                "UPDATE workers SET route_generation = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "route.delete",
                "route",
                route_id,
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )
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
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
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
                        WHERE s.target_worker_id = ?1
                          AND caller.id != ?1
                          AND caller.account_id = ?2
                          AND caller.deleted_at_ms IS NULL
                          AND d.state IN ('staging', 'validating', 'ready')
                    )",
                    params![worker_id.to_string(), account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if inbound {
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
                "UPDATE worker_routes SET state = 'tombstoned', generation = ?1,
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
