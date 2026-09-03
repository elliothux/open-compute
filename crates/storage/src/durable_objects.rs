//! Durable Object namespace authority and object-generation registry.
//!
//! Namespace and object transactions remain together because they share one SQLite authority
//! boundary and must be audited as a single generation-fencing protocol.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    BindingRepository, CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort,
    PlatformStorage, ResourceRecord, ResourceRepository, normalize_catalog_limit,
    search_as_resource_id,
};
use open_compute_core::{
    AccountId, BindingId, BindingKind, DurableObjectId, DurableObjectState, ErrorCode,
    PlatformError, ResourceId, ResourceState, VersionId, WorkerId, durable_object_namespace_prefix,
};
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;

#[path = "durable_object_migrations.rs"]
mod worker_migrations;
pub use worker_migrations::{DurableObjectClassRename, DurableObjectMigrationPlan};

/// Product schema version for P0.7 namespace rows.
pub const DO_NAMESPACE_SCHEMA_VERSION: u32 = 1;

/// Immutable Durable Object namespace product row with its resource authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableObjectNamespaceRecord {
    /// Generic resource lifecycle row.
    pub resource: ResourceRecord,
    /// Worker that owns the exported class and stable storage identity.
    pub owner_worker_id: WorkerId,
    /// Immutable named export used for dynamic facet construction.
    pub class_name: String,
    /// Stable Worker storage identity copied and checked at creation.
    pub do_storage_id: String,
    /// Opaque stable namespace storage identity.
    pub namespace_storage_key: String,
    /// Product schema version.
    pub schema_version: u32,
    /// Creation timestamp copied from the resource row.
    pub created_at_ms: i64,
}

/// One lifecycle generation in the object registry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableObjectRecord {
    /// Owning namespace resource.
    pub namespace_resource_id: ResourceId,
    /// Canonical public object identity.
    pub object_id: DurableObjectId,
    /// Monotonic generation for delete/recreate fencing.
    pub generation: u64,
    /// Current lifecycle state.
    pub state: DurableObjectState,
    /// Generation creation time.
    pub created_at_ms: i64,
    /// Last transition time.
    pub updated_at_ms: i64,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

/// One bounded page of object registry rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableObjectListPage {
    /// Rows selected for this page in deterministic order.
    pub objects: Vec<DurableObjectRecord>,
    /// Opaque cursor for the next page when more rows remain.
    pub next_cursor: Option<String>,
}

/// Encode one list cursor from the last row returned on a page.
#[must_use]
pub fn encode_object_list_cursor(record: &DurableObjectRecord) -> String {
    format!("{}:{}", record.object_id, record.generation)
}

/// Decode an opaque object-list cursor into its SQL sort key.
pub fn decode_object_list_cursor(cursor: &str) -> Result<(DurableObjectId, u64), PlatformError> {
    let (object, generation) = cursor.rsplit_once(':').ok_or_else(invalid_list_cursor)?;
    if object.is_empty() || generation.is_empty() {
        return Err(invalid_list_cursor());
    }
    let object_id = DurableObjectId::from_str(object).map_err(|_| invalid_list_cursor())?;
    let generation = generation
        .parse::<u64>()
        .map_err(|_| invalid_list_cursor())?;
    if generation == 0 {
        return Err(invalid_list_cursor());
    }
    Ok((object_id, generation))
}

/// Trusted metadata returned only to the private system-Worker router.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedDurableObjectDispatch {
    /// Account derived through binding authority.
    pub account_id: AccountId,
    /// Namespace resource resolved from the immutable binding.
    pub namespace_resource_id: ResourceId,
    /// Namespace owner Worker.
    pub worker_id: WorkerId,
    /// Current active version.
    pub version_id: VersionId,
    /// Current immutable descriptor digest.
    pub worker_code_sha256: String,
    /// Monotonic route/execution generation.
    pub route_generation: u64,
    /// Exported Durable Object class.
    pub class_name: String,
    /// Public object identity.
    pub object_id: DurableObjectId,
    /// Live object registry generation.
    pub object_generation: u64,
    /// Opaque name passed to native `idFromName()`.
    pub host_key: String,
}

/// Minimal native-delete capability derived after the object generation is fenced.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedDurableObjectDelete {
    /// Canonical public object identity used only for generation cross-checking.
    pub object_id: DurableObjectId,
    /// Fenced lifecycle generation that owns the physical host key.
    pub object_generation: u64,
    /// Opaque keyed actor identity; it cannot be selected by tenant input.
    pub host_key: String,
}

struct DispatchAuthorityRow {
    worker_id: String,
    active_version_id: String,
    route_generation: i64,
    worker_storage_id: String,
    worker_code_sha256: Vec<u8>,
    class_name: String,
    namespace_storage_id: String,
    namespace_storage_key: String,
}

struct AlarmDispatchAuthorityRow {
    account_id: String,
    worker_id: String,
    version_id: String,
    route_generation: i64,
    worker_code_sha256: Vec<u8>,
    class_name: String,
    namespace_storage_id: String,
    namespace_storage_key: String,
}

/// Durable Object repositories over central platform storage and key authority.
#[derive(Clone, Copy, Debug)]
pub struct DurableObjectRepository<'a> {
    storage: &'a PlatformStorage,
}

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

    fn namespace_is_active(&self, resource_id: ResourceId) -> Result<bool, PlatformError> {
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
    #[allow(clippy::too_many_arguments)]
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
    #[allow(clippy::too_many_arguments)]
    pub fn authorize_dispatch(
        &self,
        binding_id: BindingId,
        version_id: VersionId,
        descriptor_sha256: &[u8; 32],
        expected_route_generation: u64,
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
            if active_version_id != version_id.to_string()
                || route_generation != expected_route_generation
            {
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

type NamespaceProduct = (WorkerId, String, String, String, u32, i64);

fn collect_namespace_list_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> Result<DurableObjectNamespaceRecord, rusqlite::Error>,
    >,
) -> Result<Vec<DurableObjectNamespaceRecord>, PlatformError> {
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|_| db_error())?);
    }
    Ok(records)
}

fn map_namespace_list_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DurableObjectNamespaceRecord> {
    let worker: String = row.get(12)?;
    let schema: i64 = row.get(16)?;
    Ok(DurableObjectNamespaceRecord {
        resource: crate::resources::map_resource_offset(row, 0)?,
        owner_worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        class_name: row.get(13)?,
        do_storage_id: row.get(14)?,
        namespace_storage_key: row.get(15)?,
        schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(17)?,
    })
}

fn namespace_record(
    resource: ResourceRecord,
    product: NamespaceProduct,
) -> DurableObjectNamespaceRecord {
    DurableObjectNamespaceRecord {
        resource,
        owner_worker_id: product.0,
        class_name: product.1,
        do_storage_id: product.2,
        namespace_storage_key: product.3,
        schema_version: product.4,
        created_at_ms: product.5,
    }
}

fn read_namespace_product(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<NamespaceProduct, PlatformError> {
    conn.query_row(
        "SELECT owner_worker_id, class_name, do_storage_id, namespace_storage_key,
                    schema_version, created_at_ms FROM do_namespaces WHERE resource_id = ?1",
        [resource_id.to_string()],
        |row| {
            let worker: String = row.get(0)?;
            let schema: i64 = row.get(4)?;
            Ok((
                WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
                row.get(5)?,
            ))
        },
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(namespace_not_found)
}

fn register_object_tx(
    tx: &rusqlite::Transaction<'_>,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
    now_ms: i64,
    allow_create: bool,
) -> Result<DurableObjectRecord, PlatformError> {
    if let Some(current) = read_live_object(tx, namespace_id, object_id)? {
        return match current.state {
            DurableObjectState::Deleting => Err(PlatformError::new(
                ErrorCode::DoObjectDeleting,
                "Durable Object deletion is in progress",
            )),
            DurableObjectState::Creating => Ok(current),
            DurableObjectState::Ready => Ok(current),
            DurableObjectState::Tombstoned => Err(invariant()),
        };
    }
    if !allow_create {
        return Err(PlatformError::new(
            ErrorCode::DoStorageLimit,
            "Durable Object storage stop-writes watermark is active",
        ));
    }
    let prior: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM do_objects
             WHERE namespace_resource_id = ?1 AND object_id = ?2",
            params![namespace_id.to_string(), object_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    let generation = prior.checked_add(1).ok_or_else(invariant)?;
    tx.execute(
        "INSERT INTO do_objects
         (namespace_resource_id, object_id, generation, state,
          created_at_ms, updated_at_ms, deleted_at_ms)
         VALUES (?1, ?2, ?3, 'creating', ?4, ?4, NULL)",
        params![
            namespace_id.to_string(),
            object_id.to_string(),
            generation,
            now_ms
        ],
    )
    .map_err(|_| db_error())?;
    read_object(
        tx,
        namespace_id,
        object_id,
        u64::try_from(generation).map_err(|_| invariant())?,
    )
}

fn read_live_object(
    conn: &rusqlite::Connection,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
) -> Result<Option<DurableObjectRecord>, PlatformError> {
    conn.query_row(
        "SELECT namespace_resource_id, object_id, generation, state,
                    created_at_ms, updated_at_ms, deleted_at_ms
             FROM do_objects WHERE namespace_resource_id = ?1 AND object_id = ?2
               AND state != 'tombstoned'",
        params![namespace_id.to_string(), object_id.to_string()],
        map_object,
    )
    .optional()
    .map_err(|_| db_error())
}

fn read_object(
    conn: &rusqlite::Connection,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
    generation: u64,
) -> Result<DurableObjectRecord, PlatformError> {
    conn.query_row(
        "SELECT namespace_resource_id, object_id, generation, state,
                    created_at_ms, updated_at_ms, deleted_at_ms
             FROM do_objects WHERE namespace_resource_id = ?1 AND object_id = ?2
               AND generation = ?3",
        params![
            namespace_id.to_string(),
            object_id.to_string(),
            i64::try_from(generation).map_err(|_| invariant())?,
        ],
        map_object,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(invariant)
}

fn map_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableObjectRecord> {
    let namespace: String = row.get(0)?;
    let object: String = row.get(1)?;
    let generation: i64 = row.get(2)?;
    let state: String = row.get(3)?;
    Ok(DurableObjectRecord {
        namespace_resource_id: ResourceId::from_str(&namespace)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_id: DurableObjectId::from_str(&object).map_err(|_| rusqlite::Error::InvalidQuery)?,
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: DurableObjectState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        deleted_at_ms: row.get(6)?,
    })
}

fn namespace_storage_key(do_storage_id: &str, resource_id: ResourceId) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/do-storage/v1\0");
    digest.update(do_storage_id.as_bytes());
    digest.update(b"\0");
    digest.update(resource_id.as_uuid().as_bytes());
    hex::encode(digest.finalize())
}

fn validate_class_name(value: &str) -> Result<(), PlatformError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_class());
    };
    if value.len() > 128
        || !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$'))
        || bytes.any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')))
    {
        return Err(invalid_class());
    }
    Ok(())
}

fn array32(value: &[u8]) -> Result<[u8; 32], PlatformError> {
    value.try_into().map_err(|_| invariant())
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|_| db_error())?);
    }
    Ok(out)
}

fn invalid_class() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoClassNotFound,
        "Durable Object class name is invalid",
    )
}

fn namespace_not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoNamespaceNotFound,
        "Durable Object namespace authority was not found",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Durable Object authority invariant failed",
    )
}

fn invalid_list_cursor() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "object list cursor is invalid")
}

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Durable Object authority database operation failed",
    )
}

#[cfg(test)]
#[path = "durable_objects_tests.rs"]
mod tests;
