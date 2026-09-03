//! AI Search parent/child product authority in `control.sqlite`.

use crate::{ControlDb, ResourceRecord, ResourceRepository, resources::read_resource_conn};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, ResourceId, ResourceState,
};
use rusqlite::{OptionalExtension as _, params};
use serde::Serialize;

/// Durable product row for an AI Search namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchNamespaceRecord {
    /// Shared resource lifecycle and account authority.
    pub resource: ResourceRecord,
    /// Optional Cloudflare-facing description.
    pub description: Option<String>,
}

/// Product row for one built-in-storage AI Search instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchInstanceRecord {
    /// Shared resource lifecycle and account authority.
    pub resource: ResourceRecord,
    /// Parent namespace resource identity.
    pub namespace_resource_id: ResourceId,
    /// Cloudflare-facing `AiSearchConfig.id` identity.
    pub instance_key: String,
    /// Canonical private database locator.
    #[serde(skip)]
    pub storage_key: String,
    /// Per-instance SQLite schema version.
    pub schema_version: u32,
    /// Current model contract digest, advanced only by a fenced full reindex.
    pub model_contract_sha256: [u8; 32],
}

/// AI Search catalog repository over the central control database.
#[derive(Clone, Copy, Debug)]
pub struct AiSearchCatalog<'a> {
    db: &'a ControlDb,
}

impl<'a> AiSearchCatalog<'a> {
    /// Bind the central control authority.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Materialize a namespace locator for a creating namespace resource.
    pub fn ensure_namespace(
        self,
        resource: &ResourceRecord,
    ) -> Result<AiSearchNamespaceRecord, PlatformError> {
        self.ensure_namespace_with_description(resource, None)
    }

    /// Materialize a namespace with an optional description.
    pub fn ensure_namespace_with_description(
        self,
        resource: &ResourceRecord,
        description: Option<&str>,
    ) -> Result<AiSearchNamespaceRecord, PlatformError> {
        if resource.kind != BindingKind::AiSearchNamespace
            || resource.state != ResourceState::Creating
            || resource.driver_schema_version != 1
            || description.is_some_and(|value| value.chars().count() > 256)
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO ai_search_namespaces (resource_id, description, created_at_ms)
                 VALUES (?1, ?2, ?3) ON CONFLICT(resource_id) DO NOTHING",
                params![resource.id.to_string(), description, resource.created_at_ms],
            )
            .map_err(|_| invariant())?;
            let stored = read_namespace(tx, resource)?;
            if stored.description.as_deref() != description {
                return Err(invariant());
            }
            Ok(stored)
        })
    }

    /// Replace the optional namespace description.
    pub fn update_namespace_description(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        description: Option<&str>,
    ) -> Result<AiSearchNamespaceRecord, PlatformError> {
        if description.is_some_and(|value| value.chars().count() > 256) {
            return Err(invariant());
        }
        let resource = ResourceRepository::new(self.db).get(account_id, resource_id)?;
        if resource.kind != BindingKind::AiSearchNamespace || resource.state != ResourceState::Ready
        {
            return Err(not_found());
        }
        self.db.with_immediate(|transaction| {
            if transaction
                .execute(
                    "UPDATE ai_search_namespaces SET description=?1 WHERE resource_id=?2",
                    params![description, resource_id.to_string()],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(not_found());
            }
            read_namespace(transaction, &resource)
        })
    }

    /// Materialize an immutable child instance locator and frozen model contract.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_instance(
        self,
        resource: &ResourceRecord,
        namespace_resource_id: ResourceId,
        instance_key: &str,
        storage_key: &str,
        schema_version: u32,
        model_contract_sha256: [u8; 32],
    ) -> Result<AiSearchInstanceRecord, PlatformError> {
        validate_instance_key(instance_key)?;
        if resource.kind != BindingKind::AiSearchInstance
            || resource.state != ResourceState::Creating
            || schema_version != 1
            || schema_version != resource.driver_schema_version
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let parent = read_resource_conn(tx, resource.account_id, namespace_resource_id)?;
            if parent.kind != BindingKind::AiSearchNamespace || parent.state != ResourceState::Ready
            {
                return Err(not_found());
            }
            tx.execute(
                "INSERT INTO ai_search_instances
                 (resource_id, namespace_resource_id, instance_key, storage_key,
                  schema_version, model_contract_sha256, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(resource_id) DO NOTHING",
                params![
                    resource.id.to_string(),
                    namespace_resource_id.to_string(),
                    instance_key,
                    storage_key,
                    i64::from(schema_version),
                    model_contract_sha256,
                    resource.created_at_ms,
                ],
            )
            .map_err(|_| invariant())?;
            let stored = read_instance(tx, resource)?;
            if stored.namespace_resource_id != namespace_resource_id
                || stored.instance_key != instance_key
                || stored.storage_key != storage_key
                || stored.schema_version != schema_version
                || stored.model_contract_sha256 != model_contract_sha256
            {
                return Err(invariant());
            }
            Ok(stored)
        })
    }

    /// Read one account-scoped namespace.
    pub fn get_namespace(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<AiSearchNamespaceRecord, PlatformError> {
        let resource = ResourceRepository::new(self.db).get(account_id, resource_id)?;
        self.db
            .with_read(|connection| read_namespace(connection, &resource))
    }

    /// Read one account-scoped instance by resource identity.
    pub fn get_instance(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<AiSearchInstanceRecord, PlatformError> {
        let resource = ResourceRepository::new(self.db).get(account_id, resource_id)?;
        self.db
            .with_read(|connection| read_instance(connection, &resource))
    }

    /// Advance the control-plane model digest after the per-instance authority
    /// has durably begun the corresponding full reindex.
    pub fn update_model_contract(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        expected: [u8; 32],
        replacement: [u8; 32],
    ) -> Result<bool, PlatformError> {
        let resource = ResourceRepository::new(self.db).get(account_id, resource_id)?;
        if resource.kind != BindingKind::AiSearchInstance
            || !matches!(
                resource.state,
                ResourceState::Ready | ResourceState::Deleting
            )
        {
            return Err(invariant());
        }
        self.db.with_immediate(|transaction| {
            let updated = transaction
                .execute(
                    "UPDATE ai_search_instances SET model_contract_sha256=?1
                      WHERE resource_id=?2 AND model_contract_sha256=?3",
                    params![replacement, resource_id.to_string(), expected],
                )
                .map_err(|_| invariant())?;
            Ok(updated == 1)
        })
    }

    /// Resolve a child only within the specified account and namespace.
    pub fn get_instance_by_key(
        &self,
        account_id: AccountId,
        namespace_resource_id: ResourceId,
        instance_key: &str,
    ) -> Result<AiSearchInstanceRecord, PlatformError> {
        validate_instance_key(instance_key)?;
        self.get_namespace(account_id, namespace_resource_id)?;
        self.db.with_read(|connection| {
            let resource_id = connection
                .query_row(
                    "SELECT child.resource_id
                     FROM ai_search_instances child
                     JOIN resources resource ON resource.id = child.resource_id
                     WHERE child.namespace_resource_id=?1 AND child.instance_key=?2
                       AND resource.account_id=?3 AND resource.state != 'tombstoned'",
                    params![
                        namespace_resource_id.to_string(),
                        instance_key,
                        account_id.to_string(),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| invariant())?
                .ok_or_else(not_found)?;
            let resource_id = resource_id.parse::<ResourceId>().map_err(|_| invariant())?;
            let resource = read_resource_conn(connection, account_id, resource_id)?;
            read_instance(connection, &resource)
        })
    }

    /// List live child instances in stable instance-key order.
    pub fn list_instances(
        &self,
        account_id: AccountId,
        namespace_resource_id: ResourceId,
    ) -> Result<Vec<AiSearchInstanceRecord>, PlatformError> {
        self.get_namespace(account_id, namespace_resource_id)?;
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT child.resource_id
                     FROM ai_search_instances child
                     JOIN resources resource ON resource.id = child.resource_id
                     WHERE child.namespace_resource_id=?1 AND resource.account_id=?2
                       AND resource.state != 'tombstoned'
                     ORDER BY child.instance_key, child.resource_id",
                )
                .map_err(|_| invariant())?;
            let ids = statement
                .query_map(
                    params![namespace_resource_id.to_string(), account_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| invariant())?;
            let mut records = Vec::new();
            for id in ids {
                let id = id
                    .map_err(|_| invariant())?
                    .parse::<ResourceId>()
                    .map_err(|_| invariant())?;
                let resource = read_resource_conn(connection, account_id, id)?;
                records.push(read_instance(connection, &resource)?);
            }
            Ok(records)
        })
    }

    /// List every ready instance for bounded background maintenance.
    pub fn list_ready_instances(&self) -> Result<Vec<AiSearchInstanceRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT resource.account_id, resource.id
                       FROM ai_search_instances child
                       JOIN resources resource ON resource.id=child.resource_id
                      WHERE resource.state='ready'
                      ORDER BY resource.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| invariant())?;
            let mut records = Vec::new();
            for row in rows {
                let (account_id, resource_id) = row.map_err(|_| invariant())?;
                let account_id = account_id.parse::<AccountId>().map_err(|_| invariant())?;
                let resource_id = resource_id.parse::<ResourceId>().map_err(|_| invariant())?;
                let resource = read_resource_conn(connection, account_id, resource_id)?;
                records.push(read_instance(connection, &resource)?);
            }
            Ok(records)
        })
    }

    /// List deleting instances whose object cleanup must converge before the
    /// lifecycle driver can quarantine their local authority.
    pub fn list_deleting_instances(&self) -> Result<Vec<AiSearchInstanceRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT resource.account_id, resource.id
                       FROM ai_search_instances child
                       JOIN resources resource ON resource.id=child.resource_id
                      WHERE resource.state='deleting' ORDER BY resource.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| invariant())?;
            let mut records = Vec::new();
            for row in rows {
                let (account_id, resource_id) = row.map_err(|_| invariant())?;
                let account_id = account_id.parse::<AccountId>().map_err(|_| invariant())?;
                let resource_id = resource_id.parse::<ResourceId>().map_err(|_| invariant())?;
                let resource = read_resource_conn(connection, account_id, resource_id)?;
                records.push(read_instance(connection, &resource)?);
            }
            Ok(records)
        })
    }

    /// Return whether the namespace still owns a non-tombstoned child.
    pub fn has_live_instances(
        &self,
        account_id: AccountId,
        namespace_resource_id: ResourceId,
    ) -> Result<bool, PlatformError> {
        self.get_namespace(account_id, namespace_resource_id)?;
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM ai_search_instances child
                       JOIN resources resource ON resource.id = child.resource_id
                       WHERE child.namespace_resource_id=?1 AND resource.account_id=?2
                         AND resource.state != 'tombstoned'
                     )",
                    params![namespace_resource_id.to_string(), account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())
        })
    }
}

fn read_namespace(
    connection: &rusqlite::Connection,
    resource: &ResourceRecord,
) -> Result<AiSearchNamespaceRecord, PlatformError> {
    if resource.kind != BindingKind::AiSearchNamespace {
        return Err(not_found());
    }
    let (description, created_at_ms) = connection
        .query_row(
            "SELECT description, created_at_ms FROM ai_search_namespaces WHERE resource_id=?1",
            [resource.id.to_string()],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|_| invariant())?
        .ok_or_else(not_found)?;
    if created_at_ms != resource.created_at_ms {
        return Err(invariant());
    }
    Ok(AiSearchNamespaceRecord {
        resource: resource.clone(),
        description,
    })
}

fn read_instance(
    connection: &rusqlite::Connection,
    resource: &ResourceRecord,
) -> Result<AiSearchInstanceRecord, PlatformError> {
    if resource.kind != BindingKind::AiSearchInstance {
        return Err(not_found());
    }
    let row = connection
        .query_row(
            "SELECT namespace_resource_id, instance_key, storage_key, schema_version,
                    model_contract_sha256, created_at_ms
             FROM ai_search_instances WHERE resource_id=?1",
            [resource.id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|_| invariant())?
        .ok_or_else(not_found)?;
    if row.5 != resource.created_at_ms {
        return Err(invariant());
    }
    Ok(AiSearchInstanceRecord {
        resource: resource.clone(),
        namespace_resource_id: row.0.parse().map_err(|_| invariant())?,
        instance_key: row.1,
        storage_key: row.2,
        schema_version: u32::try_from(row.3).map_err(|_| invariant())?,
        model_contract_sha256: row.4.try_into().map_err(|_| invariant())?,
    })
}

fn validate_instance_key(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > 32
        || value.starts_with('-')
        || value.ends_with('-')
        || value.split('-').any(|segment| {
            segment.is_empty()
                || segment.bytes().any(|byte| {
                    !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                })
        })
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "AI Search instance id is invalid",
        ));
    }
    Ok(())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search catalog authority invariant failed",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "AI Search resource was not found",
    )
}
