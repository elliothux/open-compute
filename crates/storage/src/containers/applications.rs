use super::*;

impl ContainerAuthority<'_> {
    /// Register metadata only after verifying the complete image artifact chain.
    pub fn admit_image(
        &self,
        instance: InstanceId,
        image: &ContainerImage,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            insert_image(tx, image, false)
        })
    }

    /// Populate a default-scheduled application for authority fixtures.
    #[cfg(any(test, feature = "test-support"))]
    pub fn create_application(
        &self,
        instance: InstanceId,
        id: ResourceId,
        namespace: ResourceId,
        name: &str,
        max_instances: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if max_instances == 0 || !valid_application_name(name) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM do_namespaces n
                  JOIN resources r ON r.id=n.resource_id JOIN workers w ON w.id=n.owner_worker_id
                  WHERE n.resource_id=?1 AND n.lifecycle_state='active'
                    AND r.state='ready' AND w.deleted_at_ms IS NULL)",
                [namespace], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if !live { return Err(not_found()); }
            tx.execute(
                "INSERT INTO container_applications(id,namespace_resource_id,name,max_instances,created_at_ms,scheduling_policy)
                 VALUES (?1,?2,?3,?4,?5,'default') ON CONFLICT(id) DO NOTHING",
                params![id,namespace,name,max_instances,now_ms],
            ).map_err(|_| invariant())?;
            let same: bool = tx.query_row(
                "SELECT namespace_resource_id=?2 AND name=?3 AND max_instances=?4 AND scheduling_policy='default'
                 FROM container_applications WHERE id=?1",
                params![id,namespace,name,max_instances], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if !same { return Err(invariant()); }
            Ok(())
        })
    }

    /// Populate an active namespace declaration for a staging Version in authority fixtures.
    #[cfg(any(test, feature = "test-support"))]
    pub fn declare_version(
        &self,
        instance: InstanceId,
        version: VersionId,
        namespace: ResourceId,
        application_name: &str,
        descriptor: &[u8; 32],
    ) -> Result<(), PlatformError> {
        if VersionContainerDeclaration::new(namespace, application_name.to_owned())?
            .descriptor_sha256
            != *descriptor
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            tx.execute(
                "INSERT INTO version_containers(version_id,namespace_resource_id,application_name,descriptor_sha256)
                 VALUES (?1,?2,?3,?4) ON CONFLICT(version_id,namespace_resource_id) DO NOTHING",
                params![version,namespace,application_name,descriptor],
            ).map_err(|_| invariant())?;
            let same: bool = tx.query_row(
                "SELECT application_name=?3 AND descriptor_sha256=?4 FROM version_containers
                 WHERE version_id=?1 AND namespace_resource_id=?2",
                params![version,namespace,application_name,descriptor], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if !same { return Err(invariant()); }
            Ok(())
        })
    }

    /// Set admission only after provider inventory and reservations have been reconciled.
    pub fn set_admission_paused(
        &self,
        instance: InstanceId,
        application: ResourceId,
        paused: bool,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let changed = tx
                .execute(
                    "UPDATE container_applications SET admission_paused=?2
                 WHERE id=?1 AND (?2=1 OR target_generation IS NOT NULL)",
                    params![application, paused],
                )
                .map_err(|_| invariant())?;
            if changed != 1 {
                return Err(not_found());
            }
            Ok(())
        })
    }

    /// Read a lease's effective target even when the application target has advanced.
    pub fn target(
        &self,
        instance: InstanceId,
        application: ResourceId,
        generation: u64,
    ) -> Result<ContainerTarget, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            read_target(conn, application, generation)
        })
    }
}

pub(super) fn read_target(
    conn: &Connection,
    application: ResourceId,
    generation: u64,
) -> Result<ContainerTarget, PlatformError> {
    conn.query_row(
        "SELECT t.worker_version_id,t.image_sha256,t.provider_contract_sha256,t.resources_json,t.enable_internet,t.created_at_ms,a.scheduling_policy
         FROM container_application_targets t JOIN container_applications a ON a.id=t.application_id WHERE t.application_id=?1 AND t.generation=?2",
        params![application,generation],
        |row| {
            let image: Option<Vec<u8>> = row.get(1)?;
            match (row.get::<_,String>(6)?.as_str(), image.is_some()) {
                ("default",true) | ("durable_object",false) => (),
                _ => return Err(rusqlite::Error::InvalidQuery),
            }
            let contract: Vec<u8> = row.get(2)?;
            let resources: Vec<u8> = row.get(3)?;
            let resources: ContainerResources = serde_json::from_slice(&resources).map_err(|_| rusqlite::Error::InvalidQuery)?;
            resources.validate(resources).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(ContainerTarget {
                application_id: application, generation,
                spec: ContainerTargetSpec {
                    worker_version_id: row.get(0)?,
                    image_sha256: image.map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)).transpose()?,
                    provider_contract_sha256: contract.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                    resources, enable_internet: row.get(4)?,
                },
                created_at_ms: row.get(5)?,
            })
        },
    ).optional().map_err(|_| invariant())?.ok_or_else(not_found)
}

pub(super) fn publish_target_conn(
    tx: &Connection,
    application: ResourceId,
    spec: &ContainerTargetSpec,
    configuration_json: &[u8],
    now_ms: i64,
) -> Result<ContainerTarget, PlatformError> {
    if spec.image_sha256.is_none() {
        return Err(invariant());
    }
    if configuration_json.is_empty() || configuration_json.len() > 16384 {
        return Err(invariant());
    }
    let configuration: serde_json::Value =
        serde_json::from_slice(configuration_json).map_err(|_| invariant())?;
    if !configuration.is_object() {
        return Err(invariant());
    }
    let declared: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM version_containers c
                  JOIN container_applications a ON a.namespace_resource_id=c.namespace_resource_id
                    AND a.name=c.application_name
                  JOIN do_namespaces n ON n.resource_id=a.namespace_resource_id
                  JOIN worker_versions v ON v.id=c.version_id AND v.worker_id=n.owner_worker_id
                  WHERE a.id=?1 AND a.deletion_requested_at_ms IS NULL AND a.deleted_at_ms IS NULL AND c.version_id=?2
                    AND n.lifecycle_state='active' AND v.state IN ('validating','ready') AND v.deleted_at_ms IS NULL)",
                    params![application, spec.worker_version_id],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
    if !declared {
        return Err(not_found());
    }
    let prior: Option<Option<u64>> = tx
        .query_row(
            "SELECT target_generation FROM container_applications WHERE id=?1",
            [application],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| invariant())?;
    let prior = prior.ok_or_else(not_found)?;
    if let Some(generation) = prior {
        let target = read_target(tx, application, generation)?;
        let prior_json: Vec<u8> = tx.query_row(
            "SELECT configuration_json FROM container_target_configuration WHERE application_id=?1 AND target_generation=?2",
            params![application, generation], |row| row.get(0),
        ).map_err(|_| invariant())?;
        let prior_configuration: serde_json::Value =
            serde_json::from_slice(&prior_json).map_err(|_| invariant())?;
        if !prior_configuration.is_object() {
            return Err(invariant());
        }
        if prior_configuration == configuration
            && target.spec.image_sha256 == spec.image_sha256
            && target.spec.provider_contract_sha256 == spec.provider_contract_sha256
            && target.spec.resources == spec.resources
            && target.spec.enable_internet == spec.enable_internet
        {
            return Ok(target);
        }
    }
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM container_rollouts WHERE application_id=?1 AND state='running')",
        [application], |row| row.get(0),
    ).map_err(|_| invariant())?;
    if active {
        return Err(PlatformError::new(
            ErrorCode::VersionNotReady,
            "Container target cannot change during an active rollout",
        ));
    }
    let generation = prior
        .unwrap_or(0)
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(invariant)?;
    let resources = serde_json::to_vec(&spec.resources).map_err(|_| invariant())?;
    tx.execute(
        "INSERT INTO container_application_targets
                 (application_id,generation,worker_version_id,image_sha256,provider_contract_sha256,
                  resources_json,enable_internet,created_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            application,
            generation,
            spec.worker_version_id,
            spec.image_sha256,
            spec.provider_contract_sha256,
            resources,
            spec.enable_internet,
            now_ms
        ],
    )
    .map_err(|_| invariant())?;
    tx.execute(
        "INSERT INTO container_target_configuration(application_id,target_generation,configuration_json) VALUES(?1,?2,?3)",
        params![application,generation,configuration_json],
    ).map_err(|_| invariant())?;
    tx.execute(
        "UPDATE container_applications SET target_generation=?2 WHERE id=?1",
        params![application, generation],
    )
    .map_err(|_| invariant())?;
    read_target(tx, application, generation)
}

pub(super) fn insert_image(
    conn: &Connection,
    image: &ContainerImage,
    snapshot_only: bool,
) -> Result<(), PlatformError> {
    if image.diff_ids.len() > 128
        || !matches!(image.architecture.as_str(), "amd64" | "arm64")
        || image.compressed_bytes == 0
        || image.unpacked_bytes == 0
        || image.compressed_bytes > i64::MAX as u64
        || image.unpacked_bytes > i64::MAX as u64
        || image.artifact_size == 0
        || image.artifact_size > i64::MAX as u64
    {
        return Err(invariant());
    }
    let diff_ids = image.diff_ids.iter().flatten().copied().collect::<Vec<_>>();
    conn.execute("INSERT INTO container_images (manifest_sha256,config_sha256,architecture,compressed_bytes,unpacked_bytes,artifact_sha256,artifact_size,created_at_ms,diff_ids,snapshot_only) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(manifest_sha256) DO NOTHING", params![image.manifest_sha256,image.config_sha256,image.architecture,image.compressed_bytes,image.unpacked_bytes,image.artifact_sha256,image.artifact_size,image.created_at_ms,diff_ids,snapshot_only]).map_err(|_|invariant())?;
    let same:bool=conn.query_row("SELECT config_sha256=?2 AND architecture=?3 AND compressed_bytes=?4 AND unpacked_bytes=?5 AND artifact_sha256=?6 AND artifact_size=?7 AND diff_ids=?8 AND (snapshot_only=0 OR ?9=1) FROM container_images WHERE manifest_sha256=?1",params![image.manifest_sha256,image.config_sha256,image.architecture,image.compressed_bytes,image.unpacked_bytes,image.artifact_sha256,image.artifact_size,diff_ids,snapshot_only],|row|row.get(0)).map_err(|_|invariant())?;
    if !same {
        return Err(invariant());
    }
    Ok(())
}
