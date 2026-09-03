//! Read-only catalog validation shared by migration and operator diagnostics.

use super::*;
use rusqlite::Connection;

impl WorkflowRepository<'_> {
    /// Verify all catalog identities, frozen hashes, and exact typed references without repair.
    /// This exhaustive scan is for migration or explicit diagnostics, never a dispatch tick.
    pub fn verify_catalog(&self) -> Result<(), PlatformError> {
        self.db.with_read(|conn| {
            verify_catalog(conn)?;
            operations::verify_operations(conn)
        })
    }
}

pub(crate) fn verify_catalog(conn: &Connection) -> Result<(), PlatformError> {
    let integrity: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(sql_error)?;
    if integrity != "ok"
        || conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sql_error)?
    {
        return Err(invariant());
    }
    let mut definitions = conn.prepare(DEFINITION_SELECT).map_err(sql_error)?;
    for definition in definitions
        .query_map([], definition_row)
        .map_err(sql_error)?
    {
        let definition = definition.map_err(sql_error)?;
        open_compute_core::workflow::validate_workflow_name(&definition.name)
            .map_err(|_| invariant())?;
        if definition.lifecycle_generation != 1 {
            return Err(invariant());
        }
        match (
            &definition.reserved_class_name,
            &definition.reservation_owner,
            definition.reservation_state,
            definition.reservation_created_definition,
        ) {
            (Some(class_name), Some(owner), Some(_), Some(created)) => {
                validate_class_name(class_name).map_err(|_| invariant())?;
                if owner.is_empty()
                    || owner.len() > 128
                    || definition.reservation_fence < 1
                    || (created
                        && (definition.state != ResourceState::Creating
                            || definition.current_version_id.is_some()))
                {
                    return Err(invariant());
                }
            }
            (None, None, None, None) => {}
            _ => return Err(invariant()),
        }
    }
    let mut versions = conn.prepare(VERSION_SELECT).map_err(sql_error)?;
    for version in versions.query_map([], version_row).map_err(sql_error)? {
        let version = version.map_err(sql_error)?;
        if version_digest(&version.target)? != version.target.descriptor_sha256 {
            return Err(invariant());
        }
        if version.reservation_owner.is_some() != version.reservation_fence.is_some() {
            return Err(invariant());
        }
    }
    let mut bindings = conn.prepare(bindings::BINDING_SELECT).map_err(sql_error)?;
    for binding in bindings
        .query_map([], bindings::binding_row)
        .map_err(sql_error)?
    {
        let binding = binding.map_err(sql_error)?;
        if binding.descriptor.sha256()? != binding.descriptor_sha256 {
            return Err(invariant());
        }
        if binding.reservation_owner.is_some() != binding.reservation_fence.is_some() {
            return Err(invariant());
        }
    }
    let mut reservations = conn
        .prepare(instances::RESERVATION_SELECT)
        .map_err(sql_error)?;
    for reservation in reservations
        .query_map([], instances::reservation_row)
        .map_err(sql_error)?
    {
        let reservation = reservation.map_err(sql_error)?;
        let identity = reservation.identity;
        open_compute_core::workflow::validate_workflow_name(&identity.target.definition_name)
            .map_err(|_| invariant())?;
        open_compute_core::workflow::validate_workflow_instance_id(&identity.external_instance_id)
            .map_err(|_| invariant())?;
        if version_digest(&identity.target)? != identity.target.descriptor_sha256
            || identity.target.capability_version != 1
            || reservation.state == WorkflowRefState::Released
        {
            return Err(invariant());
        }
    }
    // The expected sets include timestamps. Moving tables must neither regenerate references
    // nor silently adopt dangling/mismatched references as healthy persisted authority.
    let valid: bool = conn.query_row(
        "WITH expected_version(version_id,kind,ref_id,created_at_ms) AS (
            SELECT worker_version_id,'workflow_version',id,created_at_ms FROM workflow_versions
              WHERE state NOT IN ('deleting','tombstoned')
            UNION ALL SELECT worker_version_id,'workflow_instance',instance_id,created_at_ms
              FROM workflow_instance_referrers WHERE state!='released'
         ), expected_definition(definition_id,referrer_kind,referrer_id,created_at_ms) AS (
            SELECT definition_id,'binding',id,created_at_ms FROM workflow_bindings
            UNION ALL SELECT definition_id,'instance',instance_id,created_at_ms
              FROM workflow_instance_referrers WHERE state!='released'
         )
         SELECT NOT EXISTS(SELECT * FROM expected_version EXCEPT SELECT * FROM version_referrers)
           AND NOT EXISTS(SELECT * FROM version_referrers WHERE kind IN ('workflow_version','workflow_instance')
              EXCEPT SELECT * FROM expected_version)
           AND NOT EXISTS(SELECT * FROM expected_definition EXCEPT SELECT * FROM workflow_referrers)
           AND NOT EXISTS(SELECT * FROM workflow_referrers EXCEPT SELECT * FROM expected_definition)
           AND NOT EXISTS(SELECT 1 FROM workflow_definitions f WHERE
             (f.state='ready' AND f.current_version_id IS NULL) OR
             (f.current_version_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM workflow_versions v
               WHERE v.id=f.current_version_id AND v.definition_id=f.id AND v.state='ready')))
           AND NOT EXISTS(SELECT 1 FROM workflow_versions v JOIN workflow_definitions f ON f.id=v.definition_id
             JOIN worker_versions d ON d.id=v.worker_version_id JOIN workers w ON w.id=d.worker_id
             WHERE w.account_id!=f.account_id OR w.id!=v.worker_id OR d.worker_code_sha256!=v.worker_code_sha256
               OR d.loader_schema_version!=v.loader_schema_version
               OR (v.state NOT IN ('deleting','tombstoned') AND (d.state!='ready' OR w.deleted_at_ms IS NOT NULL)))
           AND NOT EXISTS(SELECT 1 FROM workflow_bindings b JOIN workflow_definitions f ON f.id=b.definition_id
             JOIN worker_versions d ON d.id=b.version_id JOIN workers w ON w.id=d.worker_id
             WHERE f.account_id!=w.account_id OR f.lifecycle_generation!=b.definition_lifecycle_generation
               OR f.state NOT IN ('creating','ready')
               OR (f.state='creating' AND NOT (f.current_version_id IS NULL
                     AND b.reservation_owner IS NOT NULL AND b.reservation_fence IS NOT NULL
                     AND f.reserved_class_name=b.class_name
                     AND f.reservation_owner=b.reservation_owner
                     AND f.reservation_fence=b.reservation_fence)))
           AND NOT EXISTS(SELECT 1 FROM workflow_versions v JOIN workflow_definitions f ON f.id=v.definition_id
             WHERE v.state IN ('staging','validating') AND v.reservation_owner IS NOT NULL
               AND NOT (f.reserved_class_name=v.class_name AND f.reservation_owner=v.reservation_owner
                 AND f.reservation_fence=v.reservation_fence))
           AND NOT EXISTS(SELECT 1 FROM workflow_definitions f WHERE f.reservation_state='bound'
             AND NOT EXISTS(SELECT 1 FROM workflow_bindings b WHERE b.definition_id=f.id
               AND b.reservation_owner=f.reservation_owner AND b.reservation_fence=f.reservation_fence)
             AND NOT EXISTS(SELECT 1 FROM workflow_versions v WHERE v.definition_id=f.id
               AND v.reservation_owner=f.reservation_owner AND v.reservation_fence=f.reservation_fence))
           AND NOT EXISTS(SELECT 1 FROM workflow_instance_referrers r JOIN workflow_versions v ON v.id=r.workflow_version_id
             WHERE r.definition_id!=v.definition_id OR r.worker_version_id!=v.worker_version_id
               OR (r.state!='released' AND v.state!='ready'))",
        [], |row| row.get(0),
    ).map_err(sql_error)?;
    if !valid {
        return Err(invariant());
    }
    Ok(())
}
