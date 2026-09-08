use super::*;

impl<'a> DurableObjectRepository<'a> {
    /// Acknowledge that native dispatch reached the registered object generation.
    pub fn finish_object_create(
        &self,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
        generation: u64,
        now_ms: i64,
    ) -> Result<DurableObjectRecord, PlatformError> {
        self.storage.db().with_immediate(|tx| {
            let current = read_object(tx, namespace_id, object_id, generation)?;
            if current.state == DurableObjectState::Ready {
                return Ok(current);
            }
            if current.state != DurableObjectState::Creating {
                return Err(invariant());
            }
            tx.execute(
                "UPDATE do_objects SET state = 'ready', updated_at_ms = ?1
                 WHERE namespace_resource_id = ?2 AND object_id = ?3 AND generation = ?4
                   AND state = 'creating'",
                params![
                    now_ms,
                    namespace_id.to_string(),
                    object_id.to_string(),
                    i64::try_from(generation).map_err(|_| invariant())?,
                ],
            )
            .map_err(|_| db_error())?;
            read_object(tx, namespace_id, object_id, generation)
        })
    }

    /// List object generations for a namespace in deterministic order.
    pub fn list_objects(
        &self,
        account_id: AccountId,
        namespace_id: ResourceId,
    ) -> Result<Vec<DurableObjectRecord>, PlatformError> {
        self.get_namespace(account_id, namespace_id)?;
        self.storage.db().with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT namespace_resource_id, object_id, generation, state,
                            created_at_ms, updated_at_ms, deleted_at_ms
                     FROM do_objects WHERE namespace_resource_id = ?1
                     ORDER BY object_id, generation",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([namespace_id.to_string()], map_object)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// List one bounded page of object generations in deterministic order.
    pub fn list_objects_page(
        &self,
        account_id: AccountId,
        namespace_id: ResourceId,
        after: Option<(DurableObjectId, u64)>,
        limit: u16,
    ) -> Result<DurableObjectListPage, PlatformError> {
        if limit == 0 {
            return Err(invariant());
        }
        self.get_namespace(account_id, namespace_id)?;
        let fetch = u32::from(limit).saturating_add(1);
        self.storage.db().with_read(|conn| {
            let mut objects = if let Some((after_id, after_generation)) = after {
                let mut statement = conn
                    .prepare(
                        "SELECT namespace_resource_id, object_id, generation, state,
                                created_at_ms, updated_at_ms, deleted_at_ms
                         FROM do_objects
                         WHERE namespace_resource_id = ?1
                           AND (object_id > ?2 OR (object_id = ?2 AND generation > ?3))
                         ORDER BY object_id, generation
                         LIMIT ?4",
                    )
                    .map_err(|_| db_error())?;
                let rows = statement
                    .query_map(
                        params![
                            namespace_id.to_string(),
                            after_id.to_string(),
                            i64::try_from(after_generation).map_err(|_| invariant())?,
                            fetch,
                        ],
                        map_object,
                    )
                    .map_err(|_| db_error())?;
                collect_rows(rows)?
            } else {
                let mut statement = conn
                    .prepare(
                        "SELECT namespace_resource_id, object_id, generation, state,
                                created_at_ms, updated_at_ms, deleted_at_ms
                         FROM do_objects
                         WHERE namespace_resource_id = ?1
                         ORDER BY object_id, generation
                         LIMIT ?2",
                    )
                    .map_err(|_| db_error())?;
                let rows = statement
                    .query_map(params![namespace_id.to_string(), fetch], map_object)
                    .map_err(|_| db_error())?;
                collect_rows(rows)?
            };
            let next_cursor = if objects.len() > usize::from(limit) {
                objects.pop();
                objects.last().map(encode_object_list_cursor)
            } else {
                None
            };
            Ok(DurableObjectListPage {
                objects,
                next_cursor,
            })
        })
    }

    /// Read the latest registry generation for one exact object identity.
    pub fn get_latest_object(
        &self,
        account_id: AccountId,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
    ) -> Result<DurableObjectRecord, PlatformError> {
        self.get_namespace(account_id, namespace_id)?;
        if !object_id.belongs_to(namespace_id) {
            return Err(PlatformError::new(
                ErrorCode::DoIdInvalid,
                "object identity is invalid",
            ));
        }
        self.storage.db().with_read(|conn| {
            conn.query_row(
                "SELECT namespace_resource_id, object_id, generation, state,
                        created_at_ms, updated_at_ms, deleted_at_ms
                 FROM do_objects
                 WHERE namespace_resource_id = ?1 AND object_id = ?2
                 ORDER BY generation DESC
                 LIMIT 1",
                params![namespace_id.to_string(), object_id.to_string()],
                map_object,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::ResourceNotFound, "Durable Object was not found")
            })
        })
    }

    /// Fence one live object before the native facet is deleted.
    pub fn begin_object_delete(
        &self,
        account_id: AccountId,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
        now_ms: i64,
    ) -> Result<DurableObjectRecord, PlatformError> {
        self.get_namespace(account_id, namespace_id)?;
        if !object_id.belongs_to(namespace_id) {
            return Err(PlatformError::new(
                ErrorCode::DoIdInvalid,
                "object identity is invalid",
            ));
        }
        self.storage.db().with_immediate(|tx| {
            let current = read_live_object(tx, namespace_id, object_id)?;
            let Some(current) = current else {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotFound,
                    "Durable Object was not found",
                ));
            };
            if current.state == DurableObjectState::Deleting {
                return Ok(current);
            }
            if !matches!(
                current.state,
                DurableObjectState::Creating | DurableObjectState::Ready
            ) {
                return Err(invariant());
            }
            tx.execute(
                "UPDATE do_objects SET state = 'deleting', updated_at_ms = ?1
                 WHERE namespace_resource_id = ?2 AND object_id = ?3 AND generation = ?4
                   AND state IN ('creating', 'ready')",
                params![
                    now_ms,
                    namespace_id.to_string(),
                    object_id.to_string(),
                    i64::try_from(current.generation).map_err(|_| invariant())?,
                ],
            )
            .map_err(|_| db_error())?;
            read_object(tx, namespace_id, object_id, current.generation)
        })
    }

    /// Resolve trusted native-delete metadata for an already fenced object generation.
    pub fn deletion_authority(
        &self,
        account_id: AccountId,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
        generation: u64,
    ) -> Result<AuthorizedDurableObjectDelete, PlatformError> {
        let namespace = self.get_namespace(account_id, namespace_id)?;
        let object = self
            .storage
            .db()
            .with_read(|conn| read_object(conn, namespace_id, object_id, generation))?;
        if object.state != DurableObjectState::Deleting {
            return Err(PlatformError::new(
                ErrorCode::DoObjectDeleting,
                "Durable Object generation is not fenced for deletion",
            ));
        }
        Ok(AuthorizedDurableObjectDelete {
            object_id,
            object_generation: generation,
            host_key: self.storage.crypto().durable_object_host_key(
                &namespace.namespace_storage_key,
                &object_id.to_string(),
                generation,
            ),
        })
    }

    /// Mark a natively deleted object generation permanently tombstoned.
    pub fn finish_object_delete(
        &self,
        namespace_id: ResourceId,
        object_id: DurableObjectId,
        generation: u64,
        now_ms: i64,
    ) -> Result<DurableObjectRecord, PlatformError> {
        self.storage.db().with_immediate(|tx| {
            let current = read_object(tx, namespace_id, object_id, generation)?;
            if current.state == DurableObjectState::Tombstoned {
                return Ok(current);
            }
            if current.state != DurableObjectState::Deleting {
                return Err(invariant());
            }
            tx.execute(
                "UPDATE do_objects SET state = 'tombstoned', updated_at_ms = ?1,
                        deleted_at_ms = ?1
                 WHERE namespace_resource_id = ?2 AND object_id = ?3 AND generation = ?4
                   AND state = 'deleting'",
                params![
                    now_ms,
                    namespace_id.to_string(),
                    object_id.to_string(),
                    i64::try_from(generation).map_err(|_| invariant())?,
                ],
            )
            .map_err(|_| db_error())?;
            read_object(tx, namespace_id, object_id, generation)
        })
    }

    /// Return true when a namespace still has a non-tombstoned object generation.
    pub fn has_live_objects(&self, namespace_id: ResourceId) -> Result<bool, PlatformError> {
        self.storage.db().with_read(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM do_objects
                  WHERE namespace_resource_id = ?1 AND state != 'tombstoned')",
                [namespace_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|_| db_error())
        })
    }

    /// Count registered non-tombstoned host identities without inspecting native storage.
    pub fn count_live_objects(&self) -> Result<u64, PlatformError> {
        self.storage.db().with_read(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM do_objects WHERE state != 'tombstoned'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            u64::try_from(count).map_err(|_| invariant())
        })
    }

    /// Return lifecycle rows requiring crash recovery.
    pub fn reconcile_candidates(
        &self,
        limit: u32,
    ) -> Result<Vec<DurableObjectRecord>, PlatformError> {
        self.storage.db().with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT namespace_resource_id, object_id, generation, state,
                            created_at_ms, updated_at_ms, deleted_at_ms
                     FROM do_objects WHERE state IN ('creating', 'deleting')
                     ORDER BY updated_at_ms, namespace_resource_id, object_id LIMIT ?1",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([i64::from(limit)], map_object)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Scan a stable, bounded page of live object generations for alarm repair.
    pub fn alarm_repair_candidates(
        &self,
        after: Option<(ResourceId, DurableObjectId, u64)>,
        limit: u32,
    ) -> Result<Vec<DurableObjectRecord>, PlatformError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (after_namespace, after_object, after_generation) = after.map_or_else(
            || (String::new(), String::new(), 0),
            |(namespace, object, generation)| {
                (
                    namespace.to_string(),
                    object.to_string(),
                    i64::try_from(generation).unwrap_or(i64::MAX),
                )
            },
        );
        self.storage.db().with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT namespace_resource_id, object_id, generation, state,
                            created_at_ms, updated_at_ms, deleted_at_ms
                     FROM do_objects
                     WHERE state IN ('creating', 'ready') AND (
                       namespace_resource_id > ?1 OR
                       (namespace_resource_id = ?1 AND object_id > ?2) OR
                       (namespace_resource_id = ?1 AND object_id = ?2 AND generation > ?3)
                     )
                     ORDER BY namespace_resource_id, object_id, generation LIMIT ?4",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map(
                    params![
                        after_namespace,
                        after_object,
                        after_generation,
                        i64::from(limit)
                    ],
                    map_object,
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }
}
