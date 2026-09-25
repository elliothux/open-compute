use super::*;

impl WorkerRepository<'_> {
    /// Atomically create, replace, or disable the one public origin of a live tenant Worker.
    pub fn set_public_origin(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        public_name: Option<&str>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<Option<RouteRecord>, PlatformError> {
        if let Some(name) = public_name {
            validate_worker_name(name)?;
            if matches!(
                name,
                "ingress" | "ns1" | "r2" | "kv" | "api" | "admin" | "health" | "operator" | "probe"
            ) {
                return Err(PlatformError::new(
                    ErrorCode::RouteConflict,
                    "public Worker name is reserved",
                ));
            }
        }
        self.db.with_immediate(|tx| {
            let live: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workers
                     WHERE id = ?1 AND (SELECT instance_id FROM instance_identity) = ?2 AND ownership = 'tenant'
                       AND deleted_at_ms IS NULL)",
                    params![worker_id.to_string(), instance_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if !live {
                return Err(worker_not_found());
            }
            let existing = tx
                .query_row(
                    "SELECT r.id, (SELECT instance_id FROM instance_identity), r.worker_id, c.hostname_ascii,
                            r.path_prefix, r.entrypoint, r.generation, r.created_at_ms,
                            r.exposure
                     FROM worker_host_routes r
                     JOIN hostname_claims c ON c.id = r.claim_id
                     WHERE r.worker_id = ?1 AND (SELECT instance_id FROM instance_identity) = ?2
                       AND r.exposure = 'public' AND r.state = 'active'
                       AND c.state = 'active'",
                    params![worker_id.to_string(), instance_id.to_string()],
                    map_route,
                )
                .optional()
                .map_err(|_| db_error())?;
            let hostname = if let Some(name) = public_name {
                let base: String = tx
                    .query_row(
                        "SELECT d.base_domain_ascii FROM public_gateway_domains d
                         JOIN public_gateway_namespaces n ON n.domain_id = d.id
                         WHERE d.id = 1 AND d.state = 'active'
                           AND n.name = 'worker' AND n.state = 'active'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| db_error())?
                    .ok_or_else(|| {
                        PlatformError::new(
                            ErrorCode::RouteConflict,
                            "public Worker namespace is not active",
                        )
                    })?;
                let hostname = format!("{name}.{base}");
                if hostname.len() > 253 {
                    return Err(PlatformError::new(
                        ErrorCode::ConfigInvalid,
                        "public Worker hostname is too long",
                    ));
                }
                if existing
                    .as_ref()
                    .is_some_and(|route| route.hostname_ascii == hostname)
                {
                    return Ok(existing);
                }
                let occupied: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM hostname_claims
                         WHERE hostname_ascii = ?1 AND state = 'active')",
                        [&hostname],
                        |row| row.get(0),
                    )
                    .map_err(|_| db_error())?;
                if occupied {
                    return Err(PlatformError::new(
                        ErrorCode::RouteConflict,
                        "public hostname is already claimed",
                    ));
                }
                Some(hostname)
            } else {
                None
            };
            if let Some(old) = &existing {
                let changed = tx
                    .execute(
                        "UPDATE worker_host_routes
                         SET state = 'tombstoned', generation = generation + 1,
                             updated_at_ms = ?1, deleted_at_ms = ?1
                         WHERE id = ?2 AND state = 'active'",
                        params![now_ms, old.id],
                    )
                    .map_err(|_| db_error())?;
                if changed != 1 {
                    return Err(invariant());
                }
                let changed = tx
                    .execute(
                        "UPDATE hostname_claims
                     SET state = 'tombstoned', generation = generation + 1,
                         updated_at_ms = ?1, deleted_at_ms = ?1
                     WHERE id = (SELECT claim_id FROM worker_host_routes WHERE id = ?2)
                       AND state = 'active'",
                        params![now_ms, old.id],
                    )
                    .map_err(|_| db_error())?;
                if changed != 1 {
                    return Err(invariant());
                }
            }
            let result = if let Some(hostname) = hostname {
                let id = Uuid::now_v7().to_string();
                tx.execute(
                    "INSERT INTO hostname_claims
                     (id, hostname_ascii, namespace, exposure, state,
                      generation, created_at_ms, updated_at_ms, deleted_at_ms)
                     VALUES(?1, ?2, 'worker', 'public', 'active', 1, ?3, ?3, NULL)",
                    params![id, hostname, now_ms],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "INSERT INTO worker_host_routes
                     (id, claim_id, worker_id, namespace, exposure,
                      path_prefix, entrypoint, state, generation, created_at_ms,
                      updated_at_ms, deleted_at_ms)
                     VALUES(?1, ?1, ?2, 'worker', 'public', '/', NULL,
                            'active', 1, ?3, ?3, NULL)",
                    params![id, worker_id.to_string(), now_ms],
                )
                .map_err(|_| db_error())?;
                Some(RouteRecord {
                    id,
                    instance_id,
                    worker_id,
                    hostname_ascii: hostname,
                    exposure: WorkerOriginExposure::Public,
                    path_prefix: "/".to_owned(),
                    entrypoint: None,
                    generation: 1,
                    created_at_ms: now_ms,
                })
            } else {
                None
            };
            if existing.is_some() || result.is_some() {
                audit(
                    tx,
                    "worker.public_origin.set",
                    "worker",
                    &worker_id.to_string(),
                    request_id,
                    if result.is_some() {
                        br#"{"public":true}"#
                    } else {
                        br#"{"public":false}"#
                    },
                    now_ms,
                )?;
            }
            Ok(result)
        })
    }
}
