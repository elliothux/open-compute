use super::*;
use open_compute_core::ExternalOrigin;

/// Requested public origin of one tenant Worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicOriginTarget {
    /// `<name>.<base_domain>` served by the managed Gateway.
    ManagedGateway {
        /// One DNS label under the active base domain.
        name: String,
    },
    /// An operator-authorized exact origin served through the external ingress listener.
    ExternalProxy(ExternalOrigin),
}

impl WorkerRepository<'_> {
    /// Atomically create, replace, or disable the one public origin of a live tenant Worker.
    ///
    /// A request for the binding that already exists returns it unchanged.
    pub fn set_public_origin(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        target: Option<&PublicOriginTarget>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<Option<RouteRecord>, PlatformError> {
        if let Some(PublicOriginTarget::ManagedGateway { name }) = target {
            validate_worker_name(name)?;
            if matches!(
                name.as_str(),
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
                    params![worker_id, instance_id],
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
                            r.exposure, r.transport, r.external_scheme, r.external_port
                     FROM worker_host_routes r
                     JOIN hostname_claims c ON c.id = r.claim_id
                     WHERE r.worker_id = ?1 AND (SELECT instance_id FROM instance_identity) = ?2
                       AND r.exposure = 'public' AND r.state = 'active'
                       AND c.state = 'active'",
                    params![worker_id, instance_id],
                    map_route,
                )
                .optional()
                .map_err(|_| db_error())?;
            let next = match target {
                Some(target) => Some(resolve_target(tx, target)?),
                None => None,
            };
            if let (Some(old), Some((hostname, transport))) = (&existing, &next)
                && &old.hostname_ascii == hostname
                && &old.transport == transport
            {
                return Ok(existing);
            }
            if let Some((hostname, _)) = &next {
                let occupied: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM hostname_claims c
                         WHERE c.hostname_ascii = ?1 AND c.state = 'active'
                           AND c.id NOT IN (SELECT claim_id FROM worker_host_routes
                                            WHERE worker_id = ?2 AND exposure = 'public'
                                              AND state = 'active'))",
                        params![hostname, worker_id],
                        |row| row.get(0),
                    )
                    .map_err(|_| db_error())?;
                if occupied {
                    return Err(PlatformError::new(
                        ErrorCode::RouteConflict,
                        "public hostname is already claimed",
                    ));
                }
            }
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
            let result = if let Some((hostname, transport)) = next {
                let id = Uuid::now_v7().to_string();
                let (scheme, port) = match transport {
                    WorkerOriginTransport::ExternalProxy { scheme, port } => {
                        (Some(scheme.as_str()), Some(i64::from(port)))
                    }
                    WorkerOriginTransport::LocalHttp | WorkerOriginTransport::ManagedGateway => {
                        (None, None)
                    }
                };
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
                     (id, claim_id, worker_id, namespace, exposure, transport,
                      external_scheme, external_port, path_prefix, entrypoint, state,
                      generation, created_at_ms, updated_at_ms, deleted_at_ms)
                     VALUES(?1, ?1, ?2, 'worker', 'public', ?3, ?4, ?5, '/', NULL,
                            'active', 1, ?6, ?6, NULL)",
                    params![
                        id,
                        worker_id,
                        transport.ingress().as_str(),
                        scheme,
                        port,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
                Some(RouteRecord {
                    id,
                    instance_id,
                    worker_id,
                    hostname_ascii: hostname,
                    exposure: WorkerOriginExposure::Public,
                    transport,
                    path_prefix: "/".to_owned(),
                    entrypoint: None,
                    generation: 1,
                    created_at_ms: now_ms,
                })
            } else {
                None
            };
            if existing.is_some() || result.is_some() {
                let detail: &[u8] = match result.as_ref().map(|route| route.transport) {
                    Some(WorkerOriginTransport::ExternalProxy { .. }) => {
                        br#"{"public":true,"transport":"external_proxy"}"#
                    }
                    Some(_) => br#"{"public":true,"transport":"managed_gateway"}"#,
                    None => br#"{"public":false}"#,
                };
                audit(
                    tx,
                    "worker.public_origin.set",
                    "worker",
                    &worker_id.to_string(),
                    request_id,
                    detail,
                    now_ms,
                )?;
            }
            Ok(result)
        })
    }
}

fn resolve_target(
    tx: &Transaction<'_>,
    target: &PublicOriginTarget,
) -> Result<(String, WorkerOriginTransport), PlatformError> {
    let base: Option<(String, bool)> = tx
        .query_row(
            "SELECT d.base_domain_ascii,
                    d.state = 'active' AND EXISTS(
                      SELECT 1 FROM public_gateway_namespaces n
                      WHERE n.domain_id = d.id AND n.name = 'worker' AND n.state = 'active')
             FROM public_gateway_domains d WHERE d.id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| db_error())?;
    match target {
        PublicOriginTarget::ManagedGateway { name } => {
            let base = base
                .filter(|(_, active)| *active)
                .map(|(base, _)| base)
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
            Ok((hostname, WorkerOriginTransport::ManagedGateway))
        }
        PublicOriginTarget::ExternalProxy(origin) => {
            if base.is_some_and(|(base, _)| {
                origin.hostname == base || origin.hostname.ends_with(&format!(".{base}"))
            }) {
                return Err(PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "external hostname uses the managed public base domain",
                ));
            }
            Ok((
                origin.hostname.clone(),
                WorkerOriginTransport::ExternalProxy {
                    scheme: origin.scheme,
                    port: origin.port,
                },
            ))
        }
    }
}
