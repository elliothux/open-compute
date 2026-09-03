//! Durable Object class migration authority owned by immutable Worker uploads.

use super::*;
use std::collections::BTreeSet;

/// One class rename in a fixed-Wrangler Durable Object migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableObjectClassRename {
    /// Existing exported class name.
    pub from: String,
    /// Replacement exported class name retaining the same storage identity.
    pub to: String,
}

/// Closed SQLite Durable Object lifecycle plan prepared before Version validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableObjectMigrationPlan {
    /// Whether new-class declarations describe the complete exported class set.
    pub declarative: bool,
    /// Prior committed tag, absent only for the first migration.
    pub old_tag: Option<String>,
    /// New immutable migration tag.
    pub new_tag: String,
    /// New SQLite-backed classes.
    pub new_sqlite_classes: Vec<String>,
    /// Storage-preserving class renames.
    pub renamed_classes: Vec<DurableObjectClassRename>,
    /// Classes retired after the Version becomes ready.
    pub deleted_classes: Vec<String>,
}

impl DurableObjectMigrationPlan {
    /// Compute the domain-separated identity of this exact closed migration declaration.
    pub fn fingerprint(&self) -> Result<[u8; 32], PlatformError> {
        validate_migration_plan(self)?;
        let mut canonical = Vec::new();
        canonical.push(u8::from(self.declarative));
        frame(&mut canonical, self.old_tag.as_deref().unwrap_or_default())?;
        frame(&mut canonical, &self.new_tag)?;
        for class_name in &self.new_sqlite_classes {
            canonical.push(1);
            frame(&mut canonical, class_name)?;
        }
        for rename in &self.renamed_classes {
            canonical.push(2);
            frame(&mut canonical, &rename.from)?;
            frame(&mut canonical, &rename.to)?;
        }
        for class_name in &self.deleted_classes {
            canonical.push(3);
            frame(&mut canonical, class_name)?;
        }
        let mut digest = Sha256::new();
        digest.update(b"open-compute/durable-object-migration/v1");
        digest.update(canonical);
        Ok(digest.finalize().into())
    }
}

/// Durable result of preparing a migration before runtime validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableObjectMigrationPreparation {
    /// Namespace changes are pending and must publish with the Version ready transition.
    Pending,
    /// The exact plan was already published by this immutable Version.
    AlreadyCommitted {
        /// Version that already owns the exact migration plan.
        version_id: VersionId,
    },
}

/// Last committed Durable Object migration head for one Worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableObjectMigrationHead {
    /// Current immutable migration tag.
    pub tag: String,
    /// Prior committed migration tag, absent for the first migration.
    pub old_tag: Option<String>,
    /// Exact plan identity bound to the Version.
    pub plan_sha256: [u8; 32],
    /// Version whose ready transition published the migration.
    pub version_id: VersionId,
}

impl DurableObjectRepository<'_> {
    /// Prepare new/renamed namespace identities without making an unvalidated migration visible.
    pub fn prepare_worker_migration(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        plan: &DurableObjectMigrationPlan,
        now_ms: i64,
    ) -> Result<DurableObjectMigrationPreparation, PlatformError> {
        validate_migration_plan(plan)?;
        let plan_sha256 = plan.fingerprint()?;
        let max_namespaces = self.storage.hardening().max_resources_per_kind_per_account;
        self.storage.db().with_immediate(|tx| {
            let worker: Option<(String, String, Option<i64>)> = tx
                .query_row(
                    "SELECT account_id, do_storage_id, deleted_at_ms FROM workers WHERE id = ?1",
                    [worker_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((worker_account, do_storage_id, deleted_at_ms)) = worker else {
                return Err(namespace_not_found());
            };
            if deleted_at_ms.is_some() || worker_account != account_id.to_string() {
                return Err(namespace_not_found());
            }
            let current = read_migration_head(tx, worker_id)?;
            if let Some(current) = &current
                && current.tag == plan.new_tag
            {
                if current.plan_sha256 == plan_sha256 {
                    return Ok(DurableObjectMigrationPreparation::AlreadyCommitted {
                        version_id: current.version_id,
                    });
                }
                return Err(migration_conflict());
            }
            let reused_tag: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_do_migrations
                     WHERE worker_id = ?1 AND tag = ?2)",
                    params![worker_id.to_string(), plan.new_tag],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if reused_tag {
                return Err(migration_conflict());
            }
            let current_tag = current.as_ref().map(|value| value.tag.as_str());
            if current_tag != plan.old_tag.as_deref() {
                return Err(migration_conflict());
            }
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM resources
                     WHERE account_id = ?1 AND kind = 'do_namespace' AND state != 'tombstoned'",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            let mut missing = 0_i64;
            for class_name in &plan.new_sqlite_classes {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM do_namespaces
                         WHERE owner_worker_id = ?1 AND class_name = ?2)",
                        params![worker_id.to_string(), class_name],
                        |row| row.get(0),
                    )
                    .map_err(|_| db_error())?;
                if !exists {
                    missing = missing.checked_add(1).ok_or_else(invariant)?;
                }
            }
            if live_count.saturating_add(missing) > i64::from(max_namespaces) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "Durable Object namespace quota was exceeded",
                ));
            }
            for class_name in &plan.new_sqlite_classes {
                prepare_new_namespace(
                    tx,
                    account_id,
                    worker_id,
                    &do_storage_id,
                    class_name,
                    &plan.new_tag,
                    plan.declarative,
                    now_ms,
                )?;
            }
            for rename in &plan.renamed_classes {
                prepare_namespace_rename(tx, worker_id, rename, &plan.new_tag, now_ms)?;
            }
            for class_name in &plan.deleted_classes {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM do_namespaces
                         WHERE owner_worker_id = ?1 AND class_name = ?2
                           AND lifecycle_state = 'active')",
                        params![worker_id.to_string(), class_name],
                        |row| row.get(0),
                    )
                    .map_err(|_| db_error())?;
                if !exists {
                    return Err(namespace_not_found());
                }
            }
            Ok(DurableObjectMigrationPreparation::Pending)
        })
    }

    /// Resolve one active or same-migration pending class for immutable Version binding.
    pub fn namespace_for_worker_upload(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        class_name: &str,
        migration_tag: Option<&str>,
    ) -> Result<DurableObjectNamespaceRecord, PlatformError> {
        validate_class_name(class_name)?;
        let resource_id: String = self.storage.db().with_read(|conn| {
            conn.query_row(
                "SELECT n.resource_id
                 FROM do_namespaces n JOIN resources r ON r.id = n.resource_id
                 WHERE r.account_id = ?1 AND r.state = 'ready'
                   AND n.owner_worker_id = ?2 AND n.class_name = ?3
                   AND (n.lifecycle_state = 'active' OR
                        (n.lifecycle_state = 'pending' AND n.migration_tag = ?4))",
                params![
                    account_id.to_string(),
                    worker_id.to_string(),
                    class_name,
                    migration_tag,
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(namespace_not_found)
        })?;
        self.get_namespace(
            account_id,
            ResourceId::from_str(&resource_id).map_err(|_| invariant())?,
        )
    }

    /// Read the last committed Durable Object migration tag for one Worker.
    pub fn current_worker_migration(
        &self,
        worker_id: WorkerId,
    ) -> Result<Option<DurableObjectMigrationHead>, PlatformError> {
        self.storage
            .db()
            .with_read(|conn| read_migration_head(conn, worker_id))
    }

    /// Require this plan to be publishable by the selected immutable Version identity.
    pub fn validate_worker_migration_version(
        &self,
        worker_id: WorkerId,
        version_id: VersionId,
        plan: &DurableObjectMigrationPlan,
    ) -> Result<(), PlatformError> {
        validate_migration_plan(plan)?;
        let plan_sha256 = plan.fingerprint()?;
        self.storage.db().with_read(|conn| {
            let current = read_migration_head(conn, worker_id)?;
            if let Some(current) = current {
                if current.tag == plan.new_tag {
                    return if current.plan_sha256 == plan_sha256 && current.version_id == version_id
                    {
                        Ok(())
                    } else {
                        Err(migration_conflict())
                    };
                }
                if Some(current.tag.as_str()) != plan.old_tag.as_deref() {
                    return Err(migration_conflict());
                }
            } else if plan.old_tag.is_some() {
                return Err(migration_conflict());
            }
            let reused_tag: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_do_migrations
                     WHERE worker_id = ?1 AND tag = ?2)",
                    params![worker_id.to_string(), plan.new_tag],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if reused_tag {
                return Err(migration_conflict());
            }
            Ok(())
        })
    }

    /// Hide new namespaces and reverse class-name changes after Version creation fails.
    pub fn rollback_worker_migration(
        &self,
        worker_id: WorkerId,
        new_tag: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.storage.db().with_immediate(|tx| {
            let mut statement = tx
                .prepare(
                    "SELECT resource_id, class_name, previous_class_name
                     FROM do_namespaces WHERE owner_worker_id = ?1
                       AND lifecycle_state = 'pending' AND migration_tag = ?2
                     ORDER BY resource_id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map(params![worker_id.to_string(), new_tag], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|_| db_error())?;
            let mut pending = Vec::new();
            for row in rows {
                pending.push(row.map_err(|_| db_error())?);
            }
            drop(statement);
            for (resource_id, class_name, previous) in pending {
                match previous {
                    Some(previous) if previous == class_name => {
                        tx.execute(
                            "UPDATE do_namespaces SET lifecycle_state = 'retired',
                               migration_tag = NULL, previous_class_name = NULL
                             WHERE resource_id = ?1",
                            [resource_id],
                        )
                        .map_err(|_| db_error())?;
                    }
                    Some(previous) => {
                        tx.execute(
                            "UPDATE resources SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
                            params![resource_id, previous, now_ms],
                        )
                        .map_err(|_| db_error())?;
                        tx.execute(
                            "UPDATE do_namespaces SET class_name = ?2,
                               lifecycle_state = 'active', migration_tag = NULL,
                               previous_class_name = NULL WHERE resource_id = ?1",
                            params![resource_id, previous],
                        )
                        .map_err(|_| db_error())?;
                    }
                    None => {
                        tx.execute(
                            "UPDATE resources SET state = 'deleting', updated_at_ms = ?2
                             WHERE id = ?1",
                            params![resource_id, now_ms],
                        )
                        .map_err(|_| db_error())?;
                        tx.execute(
                            "UPDATE resources SET state = 'tombstoned', updated_at_ms = ?2,
                               deleted_at_ms = ?2 WHERE id = ?1",
                            params![resource_id, now_ms],
                        )
                        .map_err(|_| db_error())?;
                    }
                }
            }
            Ok(())
        })
    }
}

pub(crate) fn publish_worker_migration_tx(
    tx: &rusqlite::Transaction<'_>,
    worker_id: WorkerId,
    version_id: VersionId,
    plan: &DurableObjectMigrationPlan,
    now_ms: i64,
) -> Result<(), PlatformError> {
    validate_migration_plan(plan)?;
    let plan_sha256 = plan.fingerprint()?;
    if let Some(current) = read_migration_head(tx, worker_id)? {
        if current.tag == plan.new_tag {
            return if current.plan_sha256 == plan_sha256 && current.version_id == version_id {
                Ok(())
            } else {
                Err(migration_conflict())
            };
        }
        if Some(current.tag.as_str()) != plan.old_tag.as_deref() {
            return Err(migration_conflict());
        }
    } else if plan.old_tag.is_some() {
        return Err(migration_conflict());
    }
    let reused_tag: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_do_migrations
             WHERE worker_id = ?1 AND tag = ?2)",
            params![worker_id.to_string(), plan.new_tag],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if reused_tag {
        return Err(migration_conflict());
    }

    let mut pending_statement = tx
        .prepare(
            "SELECT class_name, previous_class_name FROM do_namespaces
             WHERE owner_worker_id = ?1 AND lifecycle_state = 'pending'
               AND migration_tag = ?2 ORDER BY class_name",
        )
        .map_err(|_| db_error())?;
    let pending_rows = pending_statement
        .query_map(params![worker_id.to_string(), plan.new_tag], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|_| db_error())?;
    let mut pending = Vec::new();
    for row in pending_rows {
        pending.push(row.map_err(|_| db_error())?);
    }
    drop(pending_statement);
    for (class_name, previous) in &pending {
        let declared = match previous {
            Some(previous) if previous != class_name => plan
                .renamed_classes
                .iter()
                .any(|rename| rename.from == *previous && rename.to == *class_name),
            Some(_) | None => plan
                .new_sqlite_classes
                .iter()
                .any(|name| name == class_name),
        };
        if !declared {
            return Err(invariant());
        }
    }
    for class_name in &plan.new_sqlite_classes {
        let lifecycle: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT lifecycle_state, migration_tag FROM do_namespaces
                 WHERE owner_worker_id = ?1 AND class_name = ?2",
                params![worker_id.to_string(), class_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| db_error())?;
        let valid = lifecycle.is_some_and(|(state, tag)| {
            (state == "pending" && tag.as_deref() == Some(plan.new_tag.as_str()))
                || (plan.declarative && state == "active" && tag.is_none())
        });
        if !valid {
            return Err(invariant());
        }
    }
    for rename in &plan.renamed_classes {
        let exact: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM do_namespaces
                 WHERE owner_worker_id = ?1 AND class_name = ?2
                   AND lifecycle_state = 'pending' AND migration_tag = ?3
                   AND previous_class_name = ?4)",
                params![worker_id.to_string(), rename.to, plan.new_tag, rename.from],
                |row| row.get(0),
            )
            .map_err(|_| db_error())?;
        if !exact {
            return Err(invariant());
        }
    }
    for class_name in &plan.deleted_classes {
        let exact: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM do_namespaces
                 WHERE owner_worker_id = ?1 AND class_name = ?2
                   AND lifecycle_state = 'active')",
                params![worker_id.to_string(), class_name],
                |row| row.get(0),
            )
            .map_err(|_| db_error())?;
        if !exact {
            return Err(invariant());
        }
    }

    tx.execute(
        "UPDATE do_namespaces
             SET lifecycle_state = 'active', migration_tag = NULL,
                 previous_class_name = NULL
             WHERE owner_worker_id = ?1 AND lifecycle_state = 'pending'
               AND migration_tag = ?2",
        params![worker_id.to_string(), plan.new_tag],
    )
    .map_err(|_| db_error())?;
    for class_name in &plan.deleted_classes {
        let changed = tx
            .execute(
                "UPDATE do_namespaces SET lifecycle_state = 'retired'
                 WHERE owner_worker_id = ?1 AND class_name = ?2
                   AND lifecycle_state = 'active'",
                params![worker_id.to_string(), class_name],
            )
            .map_err(|_| db_error())?;
        if changed != 1 {
            return Err(invariant());
        }
    }
    tx.execute(
        "INSERT INTO worker_do_migrations
         (worker_id, tag, old_tag, plan_sha256, version_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            worker_id.to_string(),
            plan.new_tag,
            plan.old_tag,
            plan_sha256.as_slice(),
            version_id.to_string(),
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    tx.execute(
        "INSERT INTO worker_do_migration_heads (worker_id, current_tag, updated_at_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(worker_id) DO UPDATE SET
           current_tag = excluded.current_tag,
           updated_at_ms = excluded.updated_at_ms",
        params![worker_id.to_string(), plan.new_tag, now_ms],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

fn read_migration_head(
    conn: &rusqlite::Connection,
    worker_id: WorkerId,
) -> Result<Option<DurableObjectMigrationHead>, PlatformError> {
    let row: Option<(String, Option<String>, Vec<u8>, String)> = conn
        .query_row(
            "SELECT h.current_tag, m.old_tag, m.plan_sha256, m.version_id
             FROM worker_do_migration_heads h
             JOIN worker_do_migrations m
               ON m.worker_id = h.worker_id AND m.tag = h.current_tag
             WHERE h.worker_id = ?1",
            [worker_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|_| db_error())?;
    row.map(|(tag, old_tag, digest, version)| {
        Ok(DurableObjectMigrationHead {
            tag,
            old_tag,
            plan_sha256: digest.try_into().map_err(|_| invariant())?,
            version_id: VersionId::from_str(&version).map_err(|_| invariant())?,
        })
    })
    .transpose()
}

fn frame(bytes: &mut Vec<u8>, value: &str) -> Result<(), PlatformError> {
    let length = u64::try_from(value.len()).map_err(|_| invariant())?;
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn migration_conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::IdempotencyConflict,
        "Durable Object migration tag conflicts with immutable Version authority",
    )
}

fn validate_migration_plan(plan: &DurableObjectMigrationPlan) -> Result<(), PlatformError> {
    if plan.new_tag.is_empty()
        || plan.new_tag.len() > 128
        || plan
            .old_tag
            .as_ref()
            .is_some_and(|tag| tag.is_empty() || tag.len() > 128)
        || plan.new_sqlite_classes.len() > 64
        || plan.renamed_classes.len() > 64
        || plan.deleted_classes.len() > 64
    {
        return Err(invariant());
    }
    let mut all = BTreeSet::new();
    for class_name in &plan.new_sqlite_classes {
        validate_class_name(class_name)?;
        if !all.insert(class_name.as_str()) {
            return Err(invariant());
        }
    }
    for rename in &plan.renamed_classes {
        validate_class_name(&rename.from)?;
        validate_class_name(&rename.to)?;
        if rename.from == rename.to
            || !all.insert(rename.from.as_str())
            || !all.insert(rename.to.as_str())
        {
            return Err(invariant());
        }
    }
    for class_name in &plan.deleted_classes {
        validate_class_name(class_name)?;
        if !all.insert(class_name.as_str()) {
            return Err(invariant());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_new_namespace(
    tx: &rusqlite::Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
    do_storage_id: &str,
    class_name: &str,
    migration_tag: &str,
    allow_active: bool,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let existing: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT resource_id, lifecycle_state, migration_tag
             FROM do_namespaces WHERE owner_worker_id = ?1 AND class_name = ?2",
            params![worker_id.to_string(), class_name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| db_error())?;
    if let Some((resource_id, lifecycle, tag)) = existing {
        return match (lifecycle.as_str(), tag.as_deref()) {
            ("active", None) if allow_active => Ok(()),
            ("pending", Some(tag)) if tag == migration_tag => Ok(()),
            ("retired", None) => {
                tx.execute(
                    "UPDATE do_namespaces SET lifecycle_state = 'pending', migration_tag = ?2,
                       previous_class_name = class_name
                     WHERE resource_id = ?1",
                    params![resource_id, migration_tag],
                )
                .map_err(|_| db_error())?;
                Ok(())
            }
            _ => Err(PlatformError::new(
                ErrorCode::ResourceNameConflict,
                "Durable Object class already exists",
            )),
        };
    }
    let resource_id = ResourceId::generate();
    tx.execute(
        "INSERT INTO resources
         (id, account_id, kind, name, state, availability, availability_code,
          spec_generation, driver_schema_version, created_at_ms, updated_at_ms, deleted_at_ms)
         VALUES (?1, ?2, 'do_namespace', ?3, 'creating', 'healthy', NULL,
                 1, ?4, ?5, ?5, NULL)",
        params![
            resource_id.to_string(),
            account_id.to_string(),
            class_name,
            i64::from(DO_NAMESPACE_SCHEMA_VERSION),
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    tx.execute(
        "INSERT INTO do_namespaces
         (resource_id, owner_worker_id, class_name, do_storage_id,
          namespace_storage_key, schema_version, lifecycle_state, migration_tag,
          previous_class_name, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, NULL, ?8)",
        params![
            resource_id.to_string(),
            worker_id.to_string(),
            class_name,
            do_storage_id,
            namespace_storage_key(do_storage_id, resource_id),
            i64::from(DO_NAMESPACE_SCHEMA_VERSION),
            migration_tag,
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    let changed = tx
        .execute(
            "UPDATE resources SET state = 'ready', updated_at_ms = ?2 WHERE id = ?1",
            params![resource_id.to_string(), now_ms],
        )
        .map_err(|_| db_error())?;
    if changed != 1 {
        return Err(invariant());
    }
    Ok(())
}

fn prepare_namespace_rename(
    tx: &rusqlite::Transaction<'_>,
    worker_id: WorkerId,
    rename: &DurableObjectClassRename,
    migration_tag: &str,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let replay: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM do_namespaces
             WHERE owner_worker_id = ?1 AND class_name = ?2
               AND lifecycle_state = 'pending' AND migration_tag = ?3
               AND previous_class_name = ?4)",
            params![worker_id.to_string(), rename.to, migration_tag, rename.from],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if replay {
        return Ok(());
    }
    let resource_id: String = tx
        .query_row(
            "SELECT resource_id FROM do_namespaces
             WHERE owner_worker_id = ?1 AND class_name = ?2 AND lifecycle_state = 'active'",
            params![worker_id.to_string(), rename.from],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?
        .ok_or_else(namespace_not_found)?;
    let target_exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM do_namespaces
             WHERE owner_worker_id = ?1 AND class_name = ?2)",
            params![worker_id.to_string(), rename.to],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if target_exists {
        return Err(PlatformError::new(
            ErrorCode::ResourceNameConflict,
            "Durable Object rename target already exists",
        ));
    }
    tx.execute(
        "UPDATE resources SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
        params![resource_id, rename.to, now_ms],
    )
    .map_err(|_| db_error())?;
    let changed = tx
        .execute(
            "UPDATE do_namespaces SET class_name = ?2, lifecycle_state = 'pending',
               migration_tag = ?3, previous_class_name = ?4 WHERE resource_id = ?1",
            params![resource_id, rename.to, migration_tag, rename.from],
        )
        .map_err(|_| db_error())?;
    if changed != 1 {
        return Err(invariant());
    }
    Ok(())
}
