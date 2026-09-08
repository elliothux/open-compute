use super::*;

impl<'a> DurableObjectRepository<'a> {
    /// Bind central storage and its master-key-derived identity authority.
    #[must_use]
    pub const fn new(storage: &'a PlatformStorage) -> Self {
        Self { storage }
    }

    /// Create or verify the immutable product row for a reserved resource.
    pub fn ensure_namespace(
        &self,
        resource: &ResourceRecord,
        owner_worker_id: WorkerId,
        class_name: &str,
    ) -> Result<DurableObjectNamespaceRecord, PlatformError> {
        validate_class_name(class_name)?;
        if resource.kind != BindingKind::DoNamespace
            || resource.state != ResourceState::Creating
            || resource.driver_schema_version != DO_NAMESPACE_SCHEMA_VERSION
        {
            return Err(invariant());
        }
        let resource_id = resource.id;
        self.storage.db().with_immediate(|tx| {
            let worker: Option<(String, String, Option<i64>)> = tx
                .query_row(
                    "SELECT account_id, do_storage_id, deleted_at_ms FROM workers WHERE id = ?1",
                    [owner_worker_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((account, do_storage_id, deleted_at_ms)) = worker else {
                return Err(PlatformError::new(
                    ErrorCode::WorkerNotFound,
                    "Durable Object namespace owner was not found",
                ));
            };
            if deleted_at_ms.is_some() || account != resource.account_id.to_string() {
                return Err(PlatformError::new(
                    ErrorCode::WorkerNotFound,
                    "Durable Object namespace owner is unavailable",
                ));
            }
            let storage_key = namespace_storage_key(&do_storage_id, resource_id);
            tx.execute(
                "INSERT INTO do_namespaces
                 (resource_id, owner_worker_id, class_name, do_storage_id,
                  namespace_storage_key, schema_version, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(resource_id) DO NOTHING",
                params![
                    resource_id.to_string(),
                    owner_worker_id.to_string(),
                    class_name,
                    do_storage_id,
                    storage_key,
                    i64::from(DO_NAMESPACE_SCHEMA_VERSION),
                    resource.created_at_ms,
                ],
            )
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::ResourceNameConflict,
                    "Worker already owns this Durable Object class",
                )
            })?;
            let row = read_namespace_product(tx, resource_id)?;
            if row.0 != owner_worker_id
                || row.1 != class_name
                || row.2 != do_storage_id
                || row.3 != storage_key
                || row.4 != DO_NAMESPACE_SCHEMA_VERSION
                || row.5 != resource.created_at_ms
            {
                return Err(invariant());
            }
            Ok(DurableObjectNamespaceRecord {
                resource: resource.clone(),
                owner_worker_id: row.0,
                class_name: row.1,
                do_storage_id: row.2,
                namespace_storage_key: row.3,
                schema_version: row.4,
                created_at_ms: row.5,
            })
        })
    }

    /// Read one namespace within its account boundary.
    pub fn get_namespace(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<DurableObjectNamespaceRecord, PlatformError> {
        let resource = ResourceRepository::new(self.storage.db()).get(account_id, resource_id)?;
        if resource.kind != BindingKind::DoNamespace {
            return Err(namespace_not_found());
        }
        let product = self
            .storage
            .db()
            .with_read(|conn| read_namespace_product(conn, resource_id))?;
        Ok(namespace_record(resource, product))
    }

    /// Read one namespace by trusted resource identity for bounded reconciliation.
    pub fn get_namespace_by_resource(
        &self,
        resource_id: ResourceId,
    ) -> Result<DurableObjectNamespaceRecord, PlatformError> {
        let account: String = self.storage.db().with_read(|conn| {
            conn.query_row(
                "SELECT account_id FROM resources WHERE id = ?1 AND kind = 'do_namespace'",
                [resource_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(namespace_not_found)
        })?;
        self.get_namespace(
            AccountId::from_str(&account).map_err(|_| invariant())?,
            resource_id,
        )
    }

    /// List all namespace resources for one account.
    pub fn list_namespaces(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<DurableObjectNamespaceRecord>, PlatformError> {
        let resources = ResourceRepository::new(self.storage.db())
            .list(account_id, Some(BindingKind::DoNamespace))?;
        resources
            .into_iter()
            .filter(|resource| resource.state != ResourceState::Tombstoned)
            .filter_map(|resource| match self.namespace_is_active(resource.id) {
                Ok(true) => Some(Ok(resource)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
            .map(|resource| {
                let resource = resource?;
                let product = self
                    .storage
                    .db()
                    .with_read(|conn| read_namespace_product(conn, resource.id))?;
                Ok(namespace_record(resource, product))
            })
            .collect()
    }

    fn namespace_is_active(self, resource_id: ResourceId) -> Result<bool, PlatformError> {
        self.storage.db().with_read(|conn| {
            conn.query_row(
                "SELECT lifecycle_state = 'active' FROM do_namespaces WHERE resource_id = ?1",
                [resource_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(namespace_not_found)
        })
    }

    /// List one bounded, filtered, and sorted page of namespace resources.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn list_namespaces_page(
        &self,
        account_id: AccountId,
        search: Option<&str>,
        status: Option<ResourceState>,
        sort: CatalogSort,
        direction: CatalogDirection,
        after: Option<CatalogCursor>,
        limit: u16,
    ) -> Result<CatalogListPage<DurableObjectNamespaceRecord>, PlatformError> {
        let limit = normalize_catalog_limit(limit);
        let fetch = u32::from(limit).saturating_add(1);
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let exact_id = search.and_then(search_as_resource_id);
        let search_needle = if exact_id.is_some() {
            None
        } else {
            search.map(str::to_lowercase)
        };
        let query = build_catalog_sql(
            "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                    r.availability_code, r.spec_generation, r.driver_schema_version,
                    r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                    n.owner_worker_id, n.class_name, n.do_storage_id,
                    n.namespace_storage_key, n.schema_version, n.created_at_ms
             FROM resources r JOIN do_namespaces n ON n.resource_id = r.id
             WHERE r.account_id = ? AND r.kind = 'do_namespace' AND r.state != 'tombstoned'
               AND n.lifecycle_state = 'active'",
            CatalogColumns {
                id: "r.id",
                name: "r.name",
                state: "r.state",
                created_at: "r.created_at_ms",
                updated_at: "r.updated_at_ms",
            },
            account_id.to_string(),
            search_needle,
            exact_id.map(|id| id.to_string()),
            status.map(|value| value.as_str().to_string()),
            sort,
            direction,
            after,
            fetch,
        )?;
        self.storage.db().with_read(|conn| {
            let mut statement = conn.prepare(&query.text).map_err(|_| db_error())?;
            let rows = statement
                .query_map(params_from_iter(query.values), map_namespace_list_row)
                .map_err(|_| db_error())?;
            let mut records = collect_namespace_list_rows(rows)?;
            let next_cursor = if records.len() > usize::from(limit) {
                records.pop();
                records.last().map(|record| {
                    record_catalog_cursor(
                        sort,
                        direction,
                        &record.resource.name,
                        record.resource.created_at_ms,
                        record.resource.updated_at_ms,
                        &record.resource.id.to_string(),
                    )
                })
            } else {
                None
            };
            Ok(CatalogListPage {
                items: records,
                next_cursor,
            })
        })
    }

    /// Return the namespace-local facade prefix and secret key.
    pub fn facade_identity(
        &self,
        resource_id: ResourceId,
    ) -> Result<([u8; 8], [u8; 32]), PlatformError> {
        let product = self
            .storage
            .db()
            .with_read(|conn| read_namespace_product(conn, resource_id))?;
        Ok((
            durable_object_namespace_prefix(resource_id),
            self.storage.crypto().durable_object_name_key(&product.3),
        ))
    }

    /// Atomically reauthorize a terminal call and register its live object generation.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn authorize_dispatch(
        &self,
        binding_id: BindingId,
        version_id: VersionId,
        descriptor_sha256: &[u8; 32],
        object_id: DurableObjectId,
        now_ms: i64,
        allow_create: bool,
    ) -> Result<AuthorizedDurableObjectDispatch, PlatformError> {
        // Reuse the canonical binding checks before the stronger active-version snapshot.
        let binding = BindingRepository::new(self.storage.db()).authorize(
            binding_id,
            version_id,
            descriptor_sha256,
        )?;
        if binding.binding.kind != BindingKind::DoNamespace
            || binding.binding.capability_version != 1
        {
            return Err(namespace_not_found());
        }
        let namespace_id = binding.resource.id;
        if !object_id.belongs_to(namespace_id) {
            return Err(PlatformError::new(
                ErrorCode::DoIdInvalid,
                "Durable Object identity belongs to another namespace",
            ));
        }
        self.storage.db().with_immediate(|tx| {
            let authority: Option<DispatchAuthorityRow> = tx
                .query_row(
                    "SELECT d.worker_id, active.version_id, w.route_generation,
                            w.do_storage_id, d.worker_code_sha256, n.class_name,
                            n.do_storage_id, n.namespace_storage_key
                     FROM version_bindings b
                     JOIN worker_versions d ON d.id = b.version_id
                     JOIN workers w ON w.id = d.worker_id
                     LEFT JOIN worker_deployments active ON active.id = w.active_deployment_id
                     JOIN do_namespaces n ON n.resource_id = b.resource_id
                     WHERE b.id = ?1 AND b.version_id = ?2 AND b.resource_id = ?3
                       AND b.descriptor_sha256 = ?4 AND d.state = 'ready'
                       AND w.deleted_at_ms IS NULL",
                    params![
                        binding_id.to_string(),
                        version_id.to_string(),
                        namespace_id.to_string(),
                        descriptor_sha256.as_slice(),
                    ],
                    |row| {
                        Ok(DispatchAuthorityRow {
                            worker_id: row.get(0)?,
                            active_version_id: row.get(1)?,
                            route_generation: row.get(2)?,
                            worker_storage_id: row.get(3)?,
                            worker_code_sha256: row.get(4)?,
                            class_name: row.get(5)?,
                            namespace_storage_id: row.get(6)?,
                            namespace_storage_key: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some(DispatchAuthorityRow {
                worker_id,
                active_version_id,
                route_generation,
                worker_storage_id,
                worker_code_sha256,
                class_name,
                namespace_storage_id,
                namespace_storage_key,
            }) = authority
            else {
                return Err(namespace_not_found());
            };
            let route_generation = u64::try_from(route_generation).map_err(|_| invariant())?;
            // A binding belongs to immutable Version code. Resolve the current route epoch
            // here, so reactivating that Version does not require rebuilding its isolate.
            // The issued authority still carries this epoch through host admission.
            if active_version_id != version_id.to_string() {
                return Err(PlatformError::new(
                    ErrorCode::DoVersionStale,
                    "Durable Object dispatch generation is stale",
                ));
            }
            if worker_storage_id != namespace_storage_id {
                return Err(invariant());
            }
            let worker_id = WorkerId::from_str(&worker_id).map_err(|_| invariant())?;
            let object = register_object_tx(tx, namespace_id, object_id, now_ms, allow_create)?;
            let host_key = self.storage.crypto().durable_object_host_key(
                &namespace_storage_key,
                &object_id.to_string(),
                object.generation,
            );
            Ok(AuthorizedDurableObjectDispatch {
                account_id: binding.account_id,
                namespace_resource_id: namespace_id,
                worker_id,
                version_id,
                worker_code_sha256: hex::encode(array32(&worker_code_sha256)?),
                route_generation,
                class_name,
                object_id,
                object_generation: object.generation,
                host_key,
            })
        })
    }

    /// Reauthorize a scheduler alarm against current namespace, object, and version authority.
    ///
    /// Unlike a public fetch, this never creates an object and does not depend on a retained
    /// version binding. The caller already holds the private scheduler capability.
    pub fn authorize_alarm_dispatch(
        &self,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
        object_generation: u64,
    ) -> Result<AuthorizedDurableObjectDispatch, PlatformError> {
        if object_generation == 0 || !object_id.belongs_to(namespace_id) {
            return Err(PlatformError::new(
                ErrorCode::DoIdInvalid,
                "Durable Object alarm identity is invalid",
            ));
        }
        self.storage.db().with_read(|connection| {
            let row: Option<AlarmDispatchAuthorityRow> = connection
                .query_row(
                    "SELECT r.account_id, n.owner_worker_id, active.version_id,
                            w.route_generation, d.worker_code_sha256, n.class_name,
                            n.do_storage_id, n.namespace_storage_key
                     FROM do_objects o
                     JOIN do_namespaces n ON n.resource_id = o.namespace_resource_id
                     JOIN resources r ON r.id = n.resource_id
                     JOIN workers w ON w.id = n.owner_worker_id
                     JOIN worker_deployments active ON active.id = w.active_deployment_id
                     JOIN worker_versions d ON d.id = active.version_id
                     WHERE o.namespace_resource_id = ?1 AND o.object_id = ?2
                       AND o.generation = ?3 AND o.state IN ('creating', 'ready')
                       AND r.state = 'ready' AND w.deleted_at_ms IS NULL AND d.state = 'ready'",
                    params![
                        namespace_id.to_string(),
                        object_id.to_string(),
                        i64::try_from(object_generation).map_err(|_| invariant())?,
                    ],
                    |row| {
                        Ok(AlarmDispatchAuthorityRow {
                            account_id: row.get(0)?,
                            worker_id: row.get(1)?,
                            version_id: row.get(2)?,
                            route_generation: row.get(3)?,
                            worker_code_sha256: row.get(4)?,
                            class_name: row.get(5)?,
                            namespace_storage_id: row.get(6)?,
                            namespace_storage_key: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some(AlarmDispatchAuthorityRow {
                account_id,
                worker_id,
                version_id,
                route_generation,
                worker_code_sha256,
                class_name,
                namespace_storage_id,
                namespace_storage_key,
            }) = row
            else {
                return Err(PlatformError::new(
                    ErrorCode::DoObjectDeleting,
                    "Durable Object alarm generation is no longer live",
                ));
            };
            let worker_storage_id: String = connection
                .query_row(
                    "SELECT do_storage_id FROM workers WHERE id = ?1",
                    [worker_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if worker_storage_id != namespace_storage_id {
                return Err(invariant());
            }
            Ok(AuthorizedDurableObjectDispatch {
                account_id: AccountId::from_str(&account_id).map_err(|_| invariant())?,
                namespace_resource_id: namespace_id,
                worker_id: WorkerId::from_str(&worker_id).map_err(|_| invariant())?,
                version_id: VersionId::from_str(&version_id).map_err(|_| invariant())?,
                worker_code_sha256: hex::encode(array32(&worker_code_sha256)?),
                route_generation: u64::try_from(route_generation).map_err(|_| invariant())?,
                class_name,
                object_id,
                object_generation,
                host_key: self.storage.crypto().durable_object_host_key(
                    &namespace_storage_key,
                    &object_id.to_string(),
                    object_generation,
                ),
            })
        })
    }
}
