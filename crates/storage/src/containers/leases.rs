use super::*;

impl ContainerAuthority<'_> {
    /// Persist native stop intent under current facet authority, or acknowledge its already stopped execution.
    pub fn begin_authorized_stop(
        &self,
        authority: &ContainerObjectAuthority,
        identity: ContainerLeaseIdentity,
        key: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let live = read_authorized_live(tx, authority)?;
            if identity.namespace_resource_id != authority.namespace_resource_id
                || identity.object_id != authority.object_id
                || identity.object_generation != authority.object_generation
            {
                return Err(invariant());
            }
            let lease = read_lease(tx, identity)?;
            if lease.application_id != authority.application_id || lease.opaque_engine_key != key {
                return Err(invariant());
            }
            if lease.state == ContainerLeaseState::Stopped {
                if lease.stop_requested_at_ms.is_none() || live.is_some() {
                    return Err(invariant());
                }
                return Ok(());
            }
            if live.as_ref().is_none_or(|live| live.identity != identity || live.opaque_engine_key != key) {
                return Err(invariant());
            }
            tx.execute("UPDATE container_instance_leases SET state='stopping',stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE opaque_engine_key=?1",params![key,now_ms]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// List durable executions for one application, including terminal history.
    pub fn application_leases(
        &self,
        instance: InstanceId,
        application: ResourceId,
    ) -> Result<Vec<ContainerLease>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let mut stmt=conn.prepare(&format!("SELECT {LEASE_COLUMNS} FROM container_instance_leases WHERE application_id=?1 ORDER BY namespace_resource_id,object_id,object_generation,lease_generation")).map_err(|_|invariant())?;
            stmt.query_map([application],decode).map_err(|_|invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|invariant())
        })
    }
    /// Resolve an existing effective lease for the current facet without reserving capacity.
    pub fn existing_lease(
        &self,
        authority: &ContainerObjectAuthority,
    ) -> Result<Option<ContainerLease>, PlatformError> {
        self.db
            .with_read(|conn| read_authorized_live(conn, authority))
    }

    /// Reserve capacity atomically, or return the same object's existing effective lease.
    /// An uncertain lease is returned for reconciliation, never replaced or double-counted.
    pub fn reserve(
        &self,
        authority: &ContainerObjectAuthority,
        engine_key: &str,
        execution: &ContainerExecutionSpec,
        instance_limit: u32,
        now_ms: i64,
    ) -> Result<ContainerLease, PlatformError> {
        if !valid_opaque(engine_key) || instance_limit == 0 {
            return Err(invariant());
        }
        execution
            .startup
            .validate_image_reference(&execution.image_sha256)?;
        directory_restores::validate_plan(&execution.directory_snapshots)?;
        self.db.with_immediate(|tx| {
            if let Some(lease) = read_authorized_live(tx,authority)? {
                if execution.image_sha256 != lease.effective_image_sha256
                    || execution.resources != lease.effective_resources || execution.enable_internet != lease.enable_internet
                    || execution.startup != lease.startup
                    || execution.directory_snapshots != lease.directory_snapshots { return Err(invariant()); }
                return Ok(lease);
            }
            let (target, max_instances, paused): (Option<u64>,u32,bool) = tx.query_row(
                "SELECT target_generation,max_instances,admission_paused FROM container_applications WHERE id=?1",
                [authority.application_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).map_err(|_| invariant())?;
            if paused { return Err(busy()); }
            let target = target.ok_or_else(not_found)?;
            let target_spec = applications::read_target(tx,authority.application_id,target)?.spec;
            if let Some(image) = target_spec.image_sha256
                && (execution.image_sha256 != image || execution.resources != target_spec.resources
                    || execution.startup.image_override || execution.startup.resources_override
                    || execution.startup.instance_profile.is_some()) { return Err(invariant()); }
            if target_spec.image_sha256.is_none() && !execution.startup.resources_override
                && execution.resources != open_compute_core::containers::ContainerInstanceProfile::Lite.resources(target_spec.resources)? {
                return Err(invariant());
            }
            execution.resources.validate(target_spec.resources)?;
            if execution.enable_internet && !target_spec.enable_internet { return Err(invariant()); }
            snapshots::validate_execution(tx,authority,execution,now_ms)?;
            directory_restores::validate_execution(tx,authority,execution,now_ms)?;
            let directories_json=serde_json::to_vec(&execution.directory_snapshots).map_err(|_|invariant())?;
            let resources_json = serde_json::to_vec(&execution.resources).map_err(|_|invariant())?;
            let startup_json = serde_json::to_vec(&execution.startup).map_err(|_|invariant())?;
            let (total, application): (u64,u64) = tx.query_row(
                "SELECT COUNT(*),COALESCE(SUM(application_id=?1),0)
                 FROM container_instance_leases WHERE state!='stopped'",
                [authority.application_id], |row| Ok((row.get(0)?,row.get(1)?)),
            ).map_err(|_| invariant())?;
            if total >= u64::from(instance_limit) || application >= u64::from(max_instances) { return Err(busy()); }
            let prior: u64 = tx.query_row(
                "SELECT COALESCE(MAX(lease_generation),0) FROM container_instance_leases
                 WHERE namespace_resource_id=?1 AND object_id=?2",
                params![authority.namespace_resource_id,authority.object_id.to_string()], |row| row.get(0),
            ).map_err(|_| invariant())?;
            let generation = prior.checked_add(1).filter(|value| *value <= i64::MAX as u64).ok_or_else(invariant)?;
            tx.execute(
                "INSERT INTO container_instance_leases
                 (application_id,namespace_resource_id,object_id,object_generation,lease_generation,
                  target_generation,opaque_engine_key,state,created_at_ms,updated_at_ms,
                  effective_image_sha256,effective_resources_json,enable_internet,startup_json,startup_envelope_json,snapshot_id,directory_restores_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'admitting',?8,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![authority.application_id,authority.namespace_resource_id,
                    authority.object_id.to_string(),authority.object_generation,generation,target,engine_key,now_ms,
                    execution.image_sha256.as_slice(),resources_json,execution.enable_internet,startup_json,execution.startup_envelope,execution.startup.snapshot_id,directories_json],
            ).map_err(|_| invariant())?;
            read_lease(tx,ContainerLeaseIdentity {
                namespace_resource_id: authority.namespace_resource_id,object_id: authority.object_id,
                object_generation: authority.object_generation,lease_generation: generation,
            })
        })
    }

    /// Fence a create/start attempt before sending any provider request.
    pub fn mark_starting(
        &self,
        identity: ContainerLeaseIdentity,
        engine_key: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.transition(identity, engine_key, ContainerLeaseState::Starting, now_ms)
    }

    /// Record uncertainty without releasing capacity or permitting a replacement start.
    pub fn mark_lost(
        &self,
        identity: ContainerLeaseIdentity,
        engine_key: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.transition(identity, engine_key, ContainerLeaseState::Lost, now_ms)
    }

    /// Reserve termination ownership before stopping the exact execution.
    pub fn begin_stop(
        &self,
        identity: ContainerLeaseIdentity,
        engine_key: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.transition(identity, engine_key, ContainerLeaseState::Stopping, now_ms)
    }

    fn transition(
        self,
        identity: ContainerLeaseIdentity,
        key: &str,
        next: ContainerLeaseState,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let current = read_lease(tx,identity)?;
            if current.opaque_engine_key != key { return Err(invariant()); }
            if current.state == next { return Ok(()); }
            let allowed = match next {
                ContainerLeaseState::Starting => current.state == ContainerLeaseState::Admitting,
                ContainerLeaseState::Lost | ContainerLeaseState::Stopping => current.state != ContainerLeaseState::Stopped,
                _ => false,
            };
            if !allowed { return Err(invariant()); }
            tx.execute(
                "UPDATE container_instance_leases SET state=?2,updated_at_ms=?3,
                   stop_requested_at_ms=CASE WHEN ?2='stopping' THEN COALESCE(stop_requested_at_ms,?3)
                     ELSE stop_requested_at_ms END WHERE opaque_engine_key=?1",
                params![key,next.as_str(),now_ms],
            ).map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Reconnect only after verifying the same provider key, image, contract, and start identity.
    pub fn observe_running(
        &self,
        identity: ContainerLeaseIdentity,
        observation: &ContainerRunningObservation<'_>,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !valid_opaque(observation.start_identity) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let lease = read_lease(tx,identity)?;
            let target = applications::read_target(tx,lease.application_id,lease.target_generation)?;
            if lease.opaque_engine_key != observation.opaque_engine_key
                || lease.stop_requested_at_ms.is_some()
                || lease.effective_image_sha256 != *observation.image_sha256
                || target.spec.provider_contract_sha256 != *observation.provider_contract_sha256
                || lease.start_identity.as_deref().is_some_and(|value| value != observation.start_identity)
                || !matches!(lease.state,ContainerLeaseState::Starting | ContainerLeaseState::Running | ContainerLeaseState::Lost) {
                return Err(invariant());
            }
            snapshots::renew_restore(tx,&lease,now_ms)?;
            tx.execute(
                "UPDATE container_instance_leases SET state='running',start_identity=?2,updated_at_ms=?3
                 WHERE opaque_engine_key=?1",
                params![observation.opaque_engine_key,observation.start_identity,now_ms],
            ).map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Atomically release capacity and object identity only after positive termination evidence.
    pub fn finish_stopped(
        &self,
        identity: ContainerLeaseIdentity,
        proof: &ContainerTermination<'_>,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !proof.terminated
            || !proof.cleanup_complete
            || !proof.late_operations_fenced
            || proof
                .start_identity
                .is_some_and(|value| !valid_opaque(value))
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let current = read_lease(tx,identity)?;
            if current.opaque_engine_key != proof.opaque_engine_key
                || current.start_identity.as_deref().is_some_and(|value| Some(value) != proof.start_identity) {
                return Err(invariant());
            }
            if current.state == ContainerLeaseState::Stopped { return Ok(()); }
            tx.execute(
                "UPDATE container_instance_leases SET state='stopped',start_identity=COALESCE(start_identity,?2),
                   stopped_at_ms=?3,updated_at_ms=?3 WHERE opaque_engine_key=?1",
                params![proof.opaque_engine_key,proof.start_identity,now_ms],
            ).map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Pause new admission and retain every unresolved execution across a daemon restart.
    pub fn lose_all(&self, instance: InstanceId, now_ms: i64) -> Result<usize, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            tx.execute("UPDATE container_applications SET admission_paused=1",[]).map_err(|_| invariant())?;
            tx.execute(
                "UPDATE container_instance_leases SET state='lost',updated_at_ms=?1 WHERE state NOT IN ('stopped','lost')",
                [now_ms],
            ).map_err(|_| invariant())
        })
    }

    /// Read one exact execution; this method does not grant a tenant control capability.
    pub fn lease(
        &self,
        instance: InstanceId,
        identity: ContainerLeaseIdentity,
    ) -> Result<ContainerLease, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            read_lease(conn, identity)
        })
    }
}

pub(super) fn read_authorized_live(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
) -> Result<Option<ContainerLease>, PlatformError> {
    require_facet_authority(conn, authority)?;
    let live = read_live(conn, authority.namespace_resource_id, authority.object_id)?;
    if live.as_ref().is_some_and(|lease| {
        lease.application_id != authority.application_id
            || lease.identity.object_generation != authority.object_generation
    }) {
        return Err(invariant());
    }
    Ok(live)
}

pub(super) fn require_facet_authority(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
) -> Result<(), PlatformError> {
    require_instance(conn, authority.instance_id)?;
    let valid: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM container_applications a
          JOIN do_namespaces n ON n.resource_id=a.namespace_resource_id
          JOIN resources r ON r.id=n.resource_id
          JOIN do_objects o ON o.namespace_resource_id=n.resource_id
          JOIN workers w ON w.id=n.owner_worker_id
          JOIN worker_deployments d ON d.id=w.active_deployment_id
          JOIN worker_versions v ON v.id=d.version_id AND v.worker_id=w.id
          JOIN version_containers c ON c.version_id=v.id AND c.namespace_resource_id=a.namespace_resource_id
            AND c.application_name=a.name
          WHERE a.id=?1 AND a.deletion_requested_at_ms IS NULL AND a.deleted_at_ms IS NULL AND n.resource_id=?2 AND o.object_id=?3 AND o.generation=?4
            AND w.route_generation=?5 AND c.descriptor_sha256=?6
            AND n.lifecycle_state='active' AND r.state='ready'
            AND o.state IN ('creating','ready') AND w.deleted_at_ms IS NULL
            AND v.state='ready' AND v.deleted_at_ms IS NULL AND d.deleted_at_ms IS NULL)",
            params![
                authority.application_id,
                authority.namespace_resource_id,
                authority.object_id.to_string(),
                authority.object_generation,
                authority.route_generation,
                authority.descriptor_sha256
            ],
            |row| row.get(0),
        )
        .map_err(|_| invariant())?;
    if !valid {
        return Err(PlatformError::new(
            ErrorCode::DoVersionStale,
            "Container facet authority is stale or undeclared",
        ));
    }
    Ok(())
}

fn busy() -> PlatformError {
    PlatformError::new(
        ErrorCode::AdmissionBusy,
        "Container admission is unavailable",
    )
}

const LEASE_COLUMNS: &str = "application_id,namespace_resource_id,object_id,object_generation,lease_generation,target_generation,opaque_engine_key,start_identity,state,created_at_ms,updated_at_ms,stopped_at_ms,stop_requested_at_ms,effective_image_sha256,effective_resources_json,enable_internet,startup_json,startup_envelope_json,snapshot_id,directory_restores_json";

fn read_live(
    conn: &Connection,
    namespace: ResourceId,
    object: DurableObjectId,
) -> Result<Option<ContainerLease>, PlatformError> {
    conn.query_row(
        &format!("SELECT {LEASE_COLUMNS} FROM container_instance_leases WHERE namespace_resource_id=?1 AND object_id=?2 AND state!='stopped'"),
        params![namespace,object.to_string()], decode,
    ).optional().map_err(|_| invariant())
}

pub(super) fn read_lease(
    conn: &Connection,
    identity: ContainerLeaseIdentity,
) -> Result<ContainerLease, PlatformError> {
    conn.query_row(
        &format!("SELECT {LEASE_COLUMNS} FROM container_instance_leases WHERE namespace_resource_id=?1 AND object_id=?2 AND object_generation=?3 AND lease_generation=?4"),
        params![identity.namespace_resource_id,identity.object_id.to_string(),identity.object_generation,identity.lease_generation], decode,
    ).optional().map_err(|_| invariant())?.ok_or_else(not_found)
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContainerLease> {
    let namespace: ResourceId = row.get(1)?;
    let object: String = row.get(2)?;
    let object: DurableObjectId = object.parse().map_err(|_| rusqlite::Error::InvalidQuery)?;
    let state: String = row.get(8)?;
    if !object.belongs_to(namespace) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let startup: ContainerLeaseStartup = serde_json::from_slice(&row.get::<_, Vec<u8>>(16)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let effective_image_sha256: [u8; 32] = row
        .get::<_, Vec<u8>>(13)?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    startup
        .validate_image_reference(&effective_image_sha256)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if startup.snapshot_id.map(|id| id.to_string()) != row.get::<_, Option<String>>(18)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let directory_snapshots: Vec<ContainerDirectoryRestore> =
        serde_json::from_slice(&row.get::<_, Vec<u8>>(19)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    directory_restores::validate_plan(&directory_snapshots)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(ContainerLease {
        directory_snapshots,
        application_id: row.get(0)?,
        identity: ContainerLeaseIdentity {
            namespace_resource_id: namespace,
            object_id: object,
            object_generation: row.get(3)?,
            lease_generation: row.get(4)?,
        },
        target_generation: row.get(5)?,
        opaque_engine_key: row.get(6)?,
        start_identity: row.get(7)?,
        state: ContainerLeaseState::parse(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
        stopped_at_ms: row.get(11)?,
        stop_requested_at_ms: row.get(12)?,
        effective_image_sha256,
        effective_resources: serde_json::from_slice(&row.get::<_, Vec<u8>>(14)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        enable_internet: row.get(15)?,
        startup_envelope: row.get(17)?,
        startup,
    })
}
