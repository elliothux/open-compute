use super::*;
use sha2::{Digest as _, Sha256};

/// Validate one official named-image map, including Unicode name length and immutable references.
pub fn validate_container_images(
    images: &std::collections::BTreeMap<String, String>,
) -> Result<(), PlatformError> {
    if images.len() > 100
        || images
            .keys()
            .any(|name| !(1..=128).contains(&name.chars().count()))
        || serde_json::to_vec(images).map_err(|_| invariant())?.len() > 16384
    {
        return Err(invariant());
    }
    for reference in images.values() {
        registry_image_digest(reference)?;
    }
    Ok(())
}

fn require_default_image_association(
    conn: &Connection,
    namespace: ResourceId,
    name: &str,
    version: VersionId,
) -> Result<(), PlatformError> {
    let named: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM version_container_images i JOIN version_containers c ON c.version_id=i.version_id AND c.namespace_resource_id=i.namespace_resource_id WHERE c.version_id=?1 AND c.namespace_resource_id=?2 AND c.application_name=?3)",
        params![version, namespace, name], |row| row.get(0),
    ).map_err(|_| invariant())?;
    if named {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "Named Container images require durable_object scheduling",
        ));
    }
    Ok(())
}

pub(crate) fn require_container_policy_associations(
    conn: &Connection,
    version: VersionId,
) -> Result<(), PlatformError> {
    let mut stmt = conn.prepare("SELECT a.namespace_resource_id,a.name FROM container_applications a JOIN version_containers c ON c.namespace_resource_id=a.namespace_resource_id AND c.application_name=a.name WHERE c.version_id=?1 AND a.scheduling_policy='default' AND a.deleted_at_ms IS NULL").map_err(|_| invariant())?;
    let associations = stmt
        .query_map([version], |row| {
            Ok((row.get::<_, ResourceId>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| invariant())?;
    for association in associations {
        let (namespace, name) = association.map_err(|_| invariant())?;
        require_default_image_association(conn, namespace, &name, version)?;
    }
    Ok(())
}

impl VersionContainerDeclaration {
    /// Extract the identity for one active-route authority check without copying image data.
    #[must_use]
    pub fn identity(&self) -> VersionContainerIdentity {
        VersionContainerIdentity {
            namespace_resource_id: self.namespace_resource_id,
            application_name: self.application_name.clone(),
            descriptor_sha256: self.descriptor_sha256,
        }
    }

    /// Normalize and bind the namespace/application association exactly once.
    pub fn new(
        namespace_resource_id: ResourceId,
        application_name: String,
    ) -> Result<Self, PlatformError> {
        Self::with_images(
            namespace_resource_id,
            application_name,
            std::collections::BTreeMap::new(),
        )
    }

    /// Bind verified named image references into the immutable Version declaration.
    pub fn with_images(
        namespace_resource_id: ResourceId,
        application_name: String,
        images: std::collections::BTreeMap<String, String>,
    ) -> Result<Self, PlatformError> {
        validate_container_images(&images)?;
        let image_bytes = serde_json::to_vec(&images).map_err(|_| invariant())?;
        if !valid_application_name(&application_name) {
            return Err(invariant());
        }
        let mut digest = Sha256::new();
        digest.update(b"open-compute-container-declaration-v2\0");
        digest.update(namespace_resource_id.to_string().as_bytes());
        digest.update(b"\0");
        digest.update(application_name.as_bytes());
        digest.update(b"\0");
        digest.update(image_bytes);
        Ok(Self {
            namespace_resource_id,
            application_name,
            images,
            descriptor_sha256: digest.finalize().into(),
        })
    }

    /// Reject corrupted persisted declarations instead of repairing them during reads.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if Self::with_images(
            self.namespace_resource_id,
            self.application_name.clone(),
            self.images.clone(),
        )? != *self
        {
            return Err(invariant());
        }
        Ok(())
    }
}

impl ContainerAuthority<'_> {
    /// Read immutable target configuration; DO policy targets have no configuration.
    pub fn target_configuration(
        &self,
        instance: InstanceId,
        application: ResourceId,
        generation: u64,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            conn.query_row("SELECT configuration_json FROM container_target_configuration WHERE application_id=?1 AND target_generation=?2",params![application,generation],|row|row.get(0)).optional().map_err(|_|invariant())?.ok_or_else(not_found)
        })
    }
    /// Read declarations for one immutable Version within the installation boundary.
    pub fn version_declarations(
        &self,
        instance: InstanceId,
        version: VersionId,
    ) -> Result<Vec<VersionContainerDeclaration>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let mut stmt = conn.prepare("SELECT namespace_resource_id,application_name,descriptor_sha256 FROM version_containers WHERE version_id=?1 ORDER BY namespace_resource_id").map_err(|_| invariant())?;
            let rows = stmt.query_map([version], |row| {
                let digest: Vec<u8> = row.get(2)?;
                Ok(VersionContainerDeclaration {
                    namespace_resource_id: row.get(0)?,
                    application_name: row.get(1)?,
                    images: std::collections::BTreeMap::new(),
                    descriptor_sha256: digest.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            }).map_err(|_| invariant())?;
            let mut declarations = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| invariant())?;
            for declaration in &mut declarations {
                let mut images = conn.prepare("SELECT image_name,image_reference,image_sha256 FROM version_container_images WHERE version_id=?1 AND namespace_resource_id=?2 ORDER BY image_name").map_err(|_| invariant())?;
                let rows = images.query_map(params![version,declaration.namespace_resource_id], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Vec<u8>>(2)?))).map_err(|_| invariant())?;
                for row in rows {
                    let (name, reference, digest) = row.map_err(|_| invariant())?;
                    if registry_image_digest(&reference)?.as_slice() != digest { return Err(invariant()); }
                    declaration.images.insert(name, reference);
                }
                declaration.validate()?;
            }
            Ok(declarations)
        })
    }

    /// List persistent applications without exposing provider identities.
    pub fn applications(
        &self,
        instance: InstanceId,
    ) -> Result<Vec<ContainerApplication>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let mut stmt = conn.prepare("SELECT id,namespace_resource_id,name,max_instances,target_generation,admission_paused,created_at_ms,scheduling_policy FROM container_applications WHERE deleted_at_ms IS NULL ORDER BY id").map_err(|_| invariant())?;
            stmt.query_map([], |row| {
                                                Ok(ContainerApplication { id: row.get(0)?, namespace_resource_id: row.get(1)?, name: row.get(2)?, max_instances: row.get(3)?, target_generation: row.get(4)?, admission_paused: row.get(5)?, created_at_ms: row.get(6)?, scheduling_policy: match row.get::<_,String>(7)?.as_str() { "default" => ContainerSchedulingPolicy::Default, "durable_object" => ContainerSchedulingPolicy::DurableObject, _ => return Err(rusqlite::Error::InvalidQuery) } })
            }).map_err(|_| invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| invariant())
        })
    }

    /// Resolve the declared application by namespace and name, never by request-supplied provider identity.
    pub fn application_for_namespace(
        &self,
        instance: InstanceId,
        namespace: ResourceId,
        name: &str,
    ) -> Result<ContainerApplication, PlatformError> {
        self.applications(instance)?
            .into_iter()
            .find(|app| app.namespace_resource_id == namespace && app.name == name)
            .ok_or_else(not_found)
    }

    /// Read the admitted image graph metadata required for private materialization.
    pub fn image(
        &self,
        instance: InstanceId,
        digest: &[u8; 32],
    ) -> Result<ContainerImage, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            read_image(conn, digest)
        })
    }

    /// Resolve an ordinary image without granting access to snapshot-only artifacts.
    pub fn ordinary_image(
        &self,
        instance: InstanceId,
        digest: &[u8; 32],
    ) -> Result<ContainerImage, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            require_ordinary_image(conn, digest)?;
            read_image(conn, digest)
        })
    }
}

pub(crate) fn insert_version_declarations(
    conn: &Connection,
    version: VersionId,
    declarations: &[VersionContainerDeclaration],
    migration: Option<&crate::durable_objects::DurableObjectMigrationPlan>,
) -> Result<(), PlatformError> {
    for declaration in declarations {
        declaration.validate()?;
        if !declaration.images.is_empty() {
            let default_application: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM container_applications WHERE namespace_resource_id=?1 AND name=?2 AND scheduling_policy='default' AND deleted_at_ms IS NULL)",
                params![declaration.namespace_resource_id,declaration.application_name], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if default_application {
                return Err(PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "Named Container images require durable_object scheduling",
                ));
            }
        }
        let (state, class, tag): (String, String, Option<String>) = conn.query_row(
            "SELECT n.lifecycle_state,n.class_name,n.migration_tag FROM do_namespaces n JOIN resources r ON r.id=n.resource_id AND r.state='ready' JOIN worker_versions v ON v.worker_id=n.owner_worker_id WHERE v.id=?1 AND n.resource_id=?2",
            params![version,declaration.namespace_resource_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).map_err(|_| invariant())?;
        let prepared_tag = match state.as_str() {
            "active" => None,
            "pending" => {
                let plan = migration.ok_or_else(invariant)?;
                if tag.as_deref() != Some(plan.new_tag.as_str())
                    || !plan.new_sqlite_classes.iter().any(|name| name == &class)
                        && !plan.renamed_classes.iter().any(|rename| rename.to == class)
                {
                    return Err(invariant());
                }
                Some(plan.new_tag.as_str())
            }
            _ => return Err(invariant()),
        };
        conn.execute("INSERT INTO version_containers(version_id,namespace_resource_id,application_name,descriptor_sha256,prepared_migration_tag) VALUES (?1,?2,?3,?4,?5)", params![version, declaration.namespace_resource_id, declaration.application_name, declaration.descriptor_sha256,prepared_tag]).map_err(|_| invariant())?;
        for (name, reference) in &declaration.images {
            let digest = registry_image_digest(reference)?;
            require_ordinary_image(conn, &digest)?;
            conn.execute("INSERT INTO version_container_images(version_id,namespace_resource_id,image_name,image_reference,image_sha256) VALUES(?1,?2,?3,?4,?5)",params![version,declaration.namespace_resource_id,name,reference,digest]).map_err(|_|invariant())?;
        }
    }
    Ok(())
}

pub(crate) fn require_ready_container_namespaces(
    conn: &Connection,
    version: VersionId,
) -> Result<(), PlatformError> {
    let invalid: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM version_containers c JOIN worker_versions v ON v.id=c.version_id WHERE c.version_id=?1 AND NOT EXISTS(SELECT 1 FROM do_namespaces n JOIN resources r ON r.id=n.resource_id WHERE n.resource_id=c.namespace_resource_id AND n.owner_worker_id=v.worker_id AND n.lifecycle_state='active' AND r.state='ready'))",
        [version], |row| row.get(0),
    ).map_err(|_| invariant())?;
    if invalid {
        return Err(invariant());
    }
    require_container_policy_associations(conn, version)
}

impl ContainerAuthority<'_> {
    /// Verify the active physical object route even before a post-upload application exists.
    pub fn resolve_declared_application(
        &self,
        instance: InstanceId,
        declaration: &VersionContainerIdentity,
        object: DurableObjectId,
        object_generation: u64,
        route_generation: u64,
    ) -> Result<Option<ContainerApplication>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let declared: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM do_namespaces n JOIN resources r ON r.id=n.resource_id AND r.state='ready' JOIN do_objects o ON o.namespace_resource_id=n.resource_id JOIN workers w ON w.id=n.owner_worker_id JOIN worker_deployments d ON d.id=w.active_deployment_id JOIN worker_versions v ON v.id=d.version_id AND v.worker_id=w.id JOIN version_containers c ON c.version_id=v.id AND c.namespace_resource_id=n.resource_id WHERE n.resource_id=?1 AND o.object_id=?2 AND o.generation=?3 AND w.route_generation=?4 AND c.application_name=?5 AND c.descriptor_sha256=?6 AND n.lifecycle_state='active' AND o.state IN ('creating','ready') AND v.state='ready' AND v.deleted_at_ms IS NULL AND w.deleted_at_ms IS NULL AND d.deleted_at_ms IS NULL)", params![declaration.namespace_resource_id,object.to_string(),object_generation,route_generation,declaration.application_name,declaration.descriptor_sha256], |row| row.get(0)).map_err(|_| invariant())?;
            if !declared { return Err(PlatformError::new(ErrorCode::DoVersionStale,"Container facet authority is stale or undeclared")); }
            Ok(())
        })?;
        Ok(self.applications(instance)?.into_iter().find(|app| {
            app.namespace_resource_id == declaration.namespace_resource_id
                && app.name == declaration.application_name
        }))
    }

    /// Resolve the active Version that authorizes an application management mutation.
    pub fn active_declaration_version(
        &self,
        instance: InstanceId,
        namespace: ResourceId,
        name: &str,
    ) -> Result<VersionId, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let id: VersionId = conn.query_row("SELECT v.id FROM do_namespaces n JOIN resources r ON r.id=n.resource_id AND r.state='ready' JOIN workers w ON w.id=n.owner_worker_id JOIN worker_deployments d ON d.id=w.active_deployment_id JOIN worker_versions v ON v.id=d.version_id AND v.worker_id=w.id JOIN version_containers c ON c.version_id=v.id AND c.namespace_resource_id=n.resource_id WHERE n.resource_id=?1 AND c.application_name=?2 AND n.lifecycle_state='active' AND v.state='ready' AND v.deleted_at_ms IS NULL AND w.deleted_at_ms IS NULL AND d.deleted_at_ms IS NULL", params![namespace,name], |row| row.get(0)).optional().map_err(|_| invariant())?.ok_or_else(not_found)?;
            Ok(id)
        })
    }

    /// Atomically update public metadata without rewriting an immutable target configuration.
    pub fn patch_application_metadata(
        &self,
        instance: InstanceId,
        application: ResourceId,
        metadata: ContainerApplicationMetadataPatch,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if metadata.max_instances == Some(0) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed = tx.execute("UPDATE container_applications SET max_instances=COALESCE(?2,max_instances) WHERE id=?1 AND deletion_requested_at_ms IS NULL AND deleted_at_ms IS NULL", params![application,metadata.max_instances]).map_err(|_| invariant())?;
            if changed != 1 { return Err(not_found()); }
            let changed = tx.execute("UPDATE container_application_settings SET observability_logs=COALESCE(?2,observability_logs),updated_at_ms=?3 WHERE application_id=?1", params![application,metadata.observability_logs,now_ms]).map_err(|_| invariant())?;
            if changed != 1 { return Err(invariant()); }
            Ok(())
        })
    }

    /// Read optional configuration bytes and metadata; DO scheduling has no default configuration.
    pub fn application_settings(
        &self,
        instance: InstanceId,
        application: ResourceId,
    ) -> Result<(Option<Vec<u8>>, bool, i64), PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            conn.query_row("SELECT configuration_json,observability_logs,updated_at_ms FROM container_application_settings WHERE application_id=?1", [application], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|_| invariant())?.ok_or_else(not_found)
        })
    }
}

impl ContainerAuthority<'_> {
    /// Atomically commit application identity, target, public configuration, capacity, and admission.
    pub fn commit_application_configuration(
        &self,
        instance: InstanceId,
        application: &ContainerApplication,
        spec: &ContainerTargetSpec,
        configuration_json: &[u8],
        metadata: ContainerApplicationMetadataPatch,
        now_ms: i64,
    ) -> Result<ContainerTarget, PlatformError> {
        if application.scheduling_policy != ContainerSchedulingPolicy::Default
            || spec.image_sha256.is_none()
        {
            return Err(invariant());
        }
        if application.max_instances == 0
            || metadata.max_instances == Some(0)
            || !valid_application_name(&application.name)
        {
            return Err(invariant());
        }
        spec.resources.validate(spec.resources)?;
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            require_default_image_association(tx, application.namespace_resource_id, &application.name, spec.worker_version_id)?;
            let active: Option<VersionId> = tx.query_row("SELECT d.version_id FROM do_namespaces n JOIN workers w ON w.id=n.owner_worker_id JOIN worker_deployments d ON d.id=w.active_deployment_id WHERE n.resource_id=?1 AND w.deleted_at_ms IS NULL AND d.deleted_at_ms IS NULL", [application.namespace_resource_id], |row| row.get(0)).optional().map_err(|_| invariant())?;
            if let Some(active) = active {
                require_default_image_association(tx, application.namespace_resource_id, &application.name, active)?;
            }
            let conflict:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM container_applications WHERE id!=?1 AND deleted_at_ms IS NULL AND (namespace_resource_id=?2 OR name=?3))",params![application.id,application.namespace_resource_id,application.name],|r|r.get(0)).map_err(|_|invariant())?;
            if conflict {return Err(PlatformError::new(ErrorCode::ResourceNameConflict,"Container application already exists"));}
            tx.execute("INSERT INTO container_applications(id,namespace_resource_id,name,max_instances,created_at_ms,scheduling_policy) VALUES(?1,?2,?3,?4,?5,'default') ON CONFLICT(id) DO NOTHING", params![application.id,application.namespace_resource_id,application.name,application.max_instances,application.created_at_ms]).map_err(|_| invariant())?;
            let same: bool = tx.query_row("SELECT namespace_resource_id=?2 AND name=?3 AND created_at_ms=?4 AND deletion_requested_at_ms IS NULL AND deleted_at_ms IS NULL AND scheduling_policy='default' FROM container_applications WHERE id=?1", params![application.id,application.namespace_resource_id,application.name,application.created_at_ms], |row|row.get(0)).map_err(|_|invariant())?;
            if !same { return Err(invariant()); }
            let target = applications::publish_target_conn(tx,application.id,spec,configuration_json,now_ms)?;
            tx.execute("INSERT INTO container_application_settings(application_id,configuration_json,observability_logs,updated_at_ms) VALUES(?1,?2,COALESCE(?3,(SELECT observability_logs FROM container_application_settings WHERE application_id=?1)),?4) ON CONFLICT(application_id) DO UPDATE SET configuration_json=excluded.configuration_json,observability_logs=excluded.observability_logs,updated_at_ms=excluded.updated_at_ms", params![application.id,configuration_json,metadata.observability_logs,now_ms]).map_err(|_|invariant())?;
            tx.execute("UPDATE container_applications SET max_instances=COALESCE(?2,max_instances),admission_paused=0 WHERE id=?1", params![application.id,metadata.max_instances]).map_err(|_|invariant())?;
            Ok(target)
        })
    }
}

pub(super) fn require_ordinary_image(
    conn: &Connection,
    digest: &[u8; 32],
) -> Result<(), PlatformError> {
    let ordinary:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM container_images WHERE manifest_sha256=?1 AND snapshot_only=0)",[digest],|row|row.get(0)).map_err(|_|invariant())?;
    if !ordinary {
        return Err(not_found());
    }
    Ok(())
}

pub(super) fn read_image(
    conn: &Connection,
    digest: &[u8; 32],
) -> Result<ContainerImage, PlatformError> {
    conn.query_row("SELECT config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,diff_ids FROM container_images WHERE manifest_sha256=?1", [digest], |row| {
                let config: Vec<u8> = row.get(0)?;
                let artifact: Vec<u8> = row.get(4)?;
                let bytes = row.get_ref(7)?.as_blob().map_err(|_| rusqlite::Error::InvalidQuery)?;
                if bytes.len() > 4096 || !bytes.len().is_multiple_of(32) {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let diff_ids = bytes.as_chunks::<32>().0.to_vec();
                Ok(ContainerImage { manifest_sha256: *digest, config_sha256: config.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, diff_ids, architecture: row.get(1)?, compressed_bytes: row.get(2)?, unpacked_bytes: row.get(3)?, artifact_sha256: artifact.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, artifact_size: row.get(5)?, created_at_ms: row.get(6)? })
            }).optional().map_err(|_| invariant())?.ok_or_else(not_found)
}
