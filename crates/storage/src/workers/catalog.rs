use super::*;

impl<'a> WorkerRepository<'a> {
    /// Return whether any instance currently owns a live Worker with this name.
    pub fn live_worker_name_exists(&self, name: &str) -> Result<bool, PlatformError> {
        validate_worker_name(name)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM workers WHERE name = ?1 AND deleted_at_ms IS NULL)",
                [name],
                |row| row.get(0),
            )
            .map_err(|_| db_error())
        })
    }

    /// Create a repository over the authoritative control database.
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Create a Worker while atomically enforcing the instance live-Worker limit.
    pub fn create_worker(
        &self,
        instance_id: InstanceId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
        max_live: u32,
    ) -> Result<(WorkerRecord, RouteRecord), PlatformError> {
        validate_worker_name(name)?;
        if is_system_reserved_worker_name(name) {
            return Err(PlatformError::new(
                ErrorCode::WorkerNameConflict,
                "Worker name is reserved for platform-owned versions",
            ));
        }
        if max_live == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Worker count limit must be greater than zero",
            ));
        }
        let worker_id = WorkerId::generate();
        let do_storage_id = Uuid::now_v7().to_string();
        let route_id = Uuid::now_v7().to_string();
        let hostname = local_worker_hostname(instance_id, name)?;
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM workers
                     WHERE (SELECT instance_id FROM instance_identity) = ?1 AND deleted_at_ms IS NULL AND ownership = 'tenant'",
                    [instance_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if live_count >= i64::from(max_live) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "instance Worker count quota was exceeded",
                ));
            }
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO workers
                 (id, name, active_deployment_id, do_storage_id,
                  route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership)
                 VALUES (?1, ?2, NULL, ?3, 1, ?4, ?4, NULL, 'tenant')",
                    params![
                        worker_id.to_string(),
                        name,
                        do_storage_id,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
            if inserted != 1 {
                return Err(PlatformError::new(
                    ErrorCode::WorkerNameConflict,
                    "a live Worker already owns this name",
                ));
            }
            tx.execute(
                "INSERT INTO worker_observability_settings
                 (worker_id, generation, enabled, head_sampling_rate, logs_enabled,
                  logs_head_sampling_rate, invocation_logs, persist, updated_at_ms)
                 VALUES (?1, 1, 1, NULL, 1, NULL, 1, 1, ?2)",
                params![worker_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO hostname_claims
                 (id, hostname_ascii, namespace, exposure, state, generation,
                  created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, 'worker', 'local', 'active', 1, ?3, ?3, NULL)",
                params![route_id, hostname, now_ms],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO worker_host_routes
                 (id, claim_id, worker_id, namespace, exposure,
                  path_prefix, entrypoint, state,
                  generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?1, ?2, 'worker', 'local', '/', NULL, 'active', 1, ?3, ?3, NULL)",
                params![route_id, worker_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                "worker.create",
                "worker",
                &worker_id.to_string(),
                request_id,
                br#"{"state":"live"}"#,
                now_ms,
            )?;
            let worker = WorkerRecord {
                id: worker_id,
                instance_id,
                name: name.to_owned(),
                active_deployment_id: None,
                active_version_id: None,
                do_storage_id: do_storage_id.clone(),
                route_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                deleted_at_ms: None,
                ownership: WorkerOwnership::Tenant,
            };
            let route = RouteRecord {
                id: route_id.clone(),
                instance_id,
                worker_id,
                hostname_ascii: hostname.clone(),
                exposure: WorkerOriginExposure::Local,
                path_prefix: "/".to_owned(),
                entrypoint: None,
                generation: 1,
                created_at_ms: now_ms,
            };
            Ok((worker, route))
        })
    }

    /// List live Workers in deterministic creation order.
    pub fn list_workers(
        &self,
        instance_id: InstanceId,
    ) -> Result<Vec<WorkerRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, (SELECT instance_id FROM instance_identity), name,
                        (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                        do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                        ownership, active_deployment_id
                 FROM workers WHERE (SELECT instance_id FROM instance_identity) = ?1 AND deleted_at_ms IS NULL
                   AND ownership = 'tenant'
                 ORDER BY created_at_ms, id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([instance_id.to_string()], map_worker)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// List one bounded, filtered, and sorted page of tenant Workers.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn list_workers_page(
        &self,
        instance_id: InstanceId,
        search: Option<&str>,
        deployed: Option<bool>,
        sort: CatalogSort,
        direction: CatalogDirection,
        after: Option<CatalogCursor>,
        limit: u16,
    ) -> Result<CatalogListPage<WorkerRecord>, PlatformError> {
        let limit = normalize_catalog_limit(limit);
        let fetch = u32::from(limit).saturating_add(1);
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let exact_id = search.and_then(search_as_worker_id);
        let search_needle = if exact_id.is_some() {
            None
        } else {
            search.map(str::to_lowercase)
        };
        let sort_expression = match sort {
            CatalogSort::Name => "name",
            CatalogSort::CreatedAt => "created_at_ms",
            CatalogSort::UpdatedAt => "updated_at_ms",
        };
        self.db.with_read(|conn| {
            let mut sql = String::from(
                "SELECT id, (SELECT instance_id FROM instance_identity), name,
                        (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                        do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                        ownership, active_deployment_id
                 FROM workers
                 WHERE (SELECT instance_id FROM instance_identity) = ? AND deleted_at_ms IS NULL AND ownership = 'tenant'",
            );
            let mut values = vec![Value::Text(instance_id.to_string())];
            if let Some(worker_id) = exact_id {
                sql.push_str(" AND id = ?");
                values.push(Value::Text(worker_id.to_string()));
            } else if let Some(needle) = search_needle {
                sql.push_str(" AND INSTR(LOWER(name), ?) > 0");
                values.push(Value::Text(needle));
            }
            if let Some(deployed) = deployed {
                sql.push_str(if deployed {
                    " AND active_deployment_id IS NOT NULL"
                } else {
                    " AND active_deployment_id IS NULL"
                });
            }
            if let Some(cursor) = after {
                if cursor.sort != sort || cursor.direction != direction {
                    return Err(invalid_catalog_cursor());
                }
                let cursor_value = match (sort, cursor.value) {
                    (CatalogSort::Name, CatalogCursorValue::Text(value)) => Value::Text(value),
                    (CatalogSort::CreatedAt | CatalogSort::UpdatedAt, CatalogCursorValue::Integer(value)) => {
                        Value::Integer(value)
                    }
                    _ => return Err(invalid_catalog_cursor()),
                };
                let comparison = direction.comparison();
                sql.push_str(&format!(
                    " AND ({sort_expression} {comparison} ? OR ({sort_expression} = ? AND id {comparison} ?))"
                ));
                values.push(cursor_value.clone());
                values.push(cursor_value);
                values.push(Value::Text(cursor.id));
            }
            sql.push_str(&format!(
                " ORDER BY {sort_expression} {}, id {} LIMIT ?",
                direction.sql(),
                direction.sql(),
            ));
            values.push(Value::Integer(i64::from(fetch)));
            let mut stmt = conn.prepare(&sql).map_err(|_| db_error())?;
            let rows = stmt
                .query_map(params_from_iter(values), map_worker)
                .map_err(|_| db_error())?;
            let mut workers = collect_rows(rows)?;
            let next_cursor = if workers.len() > usize::from(limit) {
                workers.pop();
                workers.last().map(|worker| {
                    let value = match sort {
                        CatalogSort::Name => CatalogCursorValue::Text(worker.name.clone()),
                        CatalogSort::CreatedAt => CatalogCursorValue::Integer(worker.created_at_ms),
                        CatalogSort::UpdatedAt => CatalogCursorValue::Integer(worker.updated_at_ms),
                    };
                    encode_catalog_cursor(&CatalogCursor {
                        sort,
                        direction,
                        value,
                        id: worker.id.to_string(),
                    })
                })
            } else {
                None
            };
            Ok(CatalogListPage {
                items: workers,
                next_cursor,
            })
        })
    }

    /// Read one tenant Worker and enforce its instance boundary.
    pub fn get_tenant_worker(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
    ) -> Result<WorkerRecord, PlatformError> {
        let worker = self.get_worker(instance_id, worker_id)?;
        require_tenant_worker(&worker)?;
        Ok(worker)
    }

    /// Ensure the release-owned dashboard Worker exists as a system-owned version slot.
    pub fn ensure_system_dashboard_worker(
        &self,
        instance_id: InstanceId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        if let Some(record) = self.get_system_owned_version(SystemOwnedVersionKind::Dashboard)? {
            return self.get_worker(instance_id, record.worker_id);
        }
        self.create_system_dashboard_worker(instance_id, request_id, now_ms)
    }

    /// Read one persisted system-owned version pin.
    pub fn get_system_owned_version(
        &self,
        kind: SystemOwnedVersionKind,
    ) -> Result<Option<SystemOwnedVersionRecord>, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT kind, worker_id, active_version_id, assets_sha256,
                        updated_at_ms
                 FROM system_owned_versions WHERE kind = ?1",
                [kind.as_str()],
                map_system_owned_version,
            )
            .optional()
            .map_err(|_| db_error())
        })
    }

    /// Persist the active dashboard version pin after bootstrap or promotion.
    pub fn pin_system_owned_version(
        &self,
        record: &SystemOwnedVersionRecord,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "INSERT INTO system_owned_versions
                     (kind, worker_id, active_version_id, assets_sha256, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(kind) DO UPDATE SET
                       worker_id = excluded.worker_id,
                       active_version_id = excluded.active_version_id,
                       assets_sha256 = excluded.assets_sha256,
                       updated_at_ms = excluded.updated_at_ms",
                    params![
                        record.kind.as_str(),
                        record.worker_id.to_string(),
                        record.active_version_id.as_ref().map(ToString::to_string),
                        record.assets_sha256.as_slice(),
                        record.updated_at_ms,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            Ok(())
        })
    }

    fn create_system_dashboard_worker(
        self,
        instance_id: InstanceId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        let worker_id = WorkerId::generate();
        let do_storage_id = Uuid::now_v7().to_string();
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let inserted = tx
                .execute(
                    "INSERT INTO workers
                     (id, name, active_deployment_id, do_storage_id,
                      route_generation, created_at_ms, updated_at_ms, deleted_at_ms, ownership)
                     VALUES (?1, ?2, NULL, ?3, 1, ?4, ?4, NULL, 'system')",
                    params![
                        worker_id.to_string(),
                        SYSTEM_DASHBOARD_WORKER_NAME,
                        do_storage_id,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
            if inserted != 1 {
                return Err(invariant());
            }
            tx.execute(
                "INSERT INTO worker_observability_settings
                 (worker_id, generation, enabled, head_sampling_rate, logs_enabled,
                  logs_head_sampling_rate, invocation_logs, persist, updated_at_ms)
                 VALUES (?1, 1, 0, NULL, 0, NULL, 0, 0, ?2)",
                params![worker_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO system_owned_versions
                 (kind, worker_id, active_version_id, assets_sha256, updated_at_ms)
                 VALUES ('dashboard', ?1, NULL, zeroblob(32), ?2)",
                params![worker_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                "worker.create.system",
                "worker",
                &worker_id.to_string(),
                request_id,
                br#"{"state":"system","name":"open-compute-dashboard"}"#,
                now_ms,
            )?;
            Ok(WorkerRecord {
                id: worker_id,
                instance_id,
                name: SYSTEM_DASHBOARD_WORKER_NAME.to_owned(),
                active_deployment_id: None,
                active_version_id: None,
                do_storage_id,
                route_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                deleted_at_ms: None,
                ownership: WorkerOwnership::System,
            })
        })
    }

    /// Read one Worker and enforce its instance boundary.
    pub fn get_worker(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
    ) -> Result<WorkerRecord, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT id, (SELECT instance_id FROM instance_identity), name,
                        (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                        do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                        ownership, active_deployment_id
                 FROM workers WHERE id = ?1 AND (SELECT instance_id FROM instance_identity) = ?2",
                params![worker_id.to_string(), instance_id.to_string()],
                map_worker,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(worker_not_found)
        })
    }

    /// Read the current Script-level Workers Logs policy.
    pub fn get_observability_settings(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
    ) -> Result<WorkerObservabilitySettings, PlatformError> {
        self.get_worker(instance_id, worker_id)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT generation, enabled, head_sampling_rate, logs_enabled,
                        logs_head_sampling_rate, invocation_logs, persist, updated_at_ms
                 FROM worker_observability_settings WHERE worker_id = ?1",
                [worker_id.to_string()],
                map_observability_settings,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(invariant)
        })
    }

    /// Atomically replace one Script policy and invalidate every warm runtime key.
    pub fn update_observability_settings(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        expected_route_generation: u64,
        settings: &UpdateWorkerObservabilitySettings,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerObservabilitySettings, PlatformError> {
        validate_sampling_rate(settings.head_sampling_rate)?;
        validate_sampling_rate(settings.logs_head_sampling_rate)?;
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, instance_id, worker_id)?;
            require_tenant_worker(&worker)?;
            if worker.route_generation != expected_route_generation {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Worker route generation changed before observability update",
                ));
            }
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
            let route_generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            let changed = tx
                .execute(
                    "UPDATE worker_observability_settings SET generation=?1, enabled=?2,
                       head_sampling_rate=?3, logs_enabled=?4, logs_head_sampling_rate=?5,
                       invocation_logs=?6, persist=?7, updated_at_ms=?8
                     WHERE worker_id=?9 AND generation=?10",
                    params![
                        i64::try_from(generation).map_err(|_| invariant())?,
                        settings.enabled,
                        settings.head_sampling_rate,
                        settings.logs_enabled,
                        settings.logs_head_sampling_rate,
                        settings.invocation_logs,
                        settings.persist,
                        now_ms,
                        worker_id.to_string(),
                        i64::try_from(current.generation).map_err(|_| invariant())?,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            tx.execute(
                "UPDATE workers SET route_generation=?1, updated_at_ms=?2
                 WHERE id=?3 AND route_generation=?4",
                params![
                    i64::try_from(route_generation).map_err(|_| invariant())?,
                    now_ms,
                    worker_id.to_string(),
                    i64::try_from(worker.route_generation).map_err(|_| invariant())?,
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                "worker.observability.update",
                "worker",
                &worker_id.to_string(),
                request_id,
                format!(r#"{{"generation":{generation}}}"#).as_bytes(),
                now_ms,
            )?;
            Ok(WorkerObservabilitySettings {
                generation,
                enabled: settings.enabled,
                head_sampling_rate: settings.head_sampling_rate,
                logs_enabled: settings.logs_enabled,
                logs_head_sampling_rate: settings.logs_head_sampling_rate,
                invocation_logs: settings.invocation_logs,
                persist: settings.persist,
                updated_at_ms: now_ms,
            })
        })
    }

    /// Append one bounded, content-free observability management audit event.
    pub fn audit_observability(
        &self,
        instance_id: InstanceId,
        event: &ObservabilityAudit,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let (action, target_type, target_id, details) = match event {
            ObservabilityAudit::TailCreate { worker_id } => (
                "worker.tail.create",
                "worker",
                worker_id.to_string(),
                serde_json::json!({}),
            ),
            ObservabilityAudit::TailDelete { worker_id } => (
                "worker.tail.delete",
                "worker",
                worker_id.to_string(),
                serde_json::json!({}),
            ),
            ObservabilityAudit::Query {
                view,
                from_ms,
                to_ms,
                result_count,
                filter_keys,
            } => {
                if !matches!(view.as_str(), "events" | "invocations")
                    || from_ms >= to_ms
                    || filter_keys.len() > 32
                    || filter_keys
                        .iter()
                        .any(|key| key.is_empty() || key.len() > 512)
                {
                    return Err(invariant());
                }
                (
                    "worker.observability.query",
                    "instance",
                    instance_id.to_string(),
                    serde_json::json!({
                        "view": view,
                        "fromMs": from_ms,
                        "toMs": to_ms,
                        "resultCount": result_count,
                        "filterKeys": filter_keys,
                    }),
                )
            }
        };
        let details = serde_json::to_vec(&details).map_err(|_| invariant())?;
        self.db.with_immediate(|tx| {
            audit(
                tx,
                action,
                target_type,
                &target_id,
                request_id,
                &details,
                now_ms,
            )
        })
    }
}
