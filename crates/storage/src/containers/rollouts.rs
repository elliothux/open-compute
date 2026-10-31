use super::*;

fn valid_steps(steps: &[u32]) -> bool {
    !steps.is_empty()
        && steps.len() <= 100
        && steps.last() == Some(&100)
        && steps.iter().all(|step| (1..=100).contains(step))
        && steps.windows(2).all(|pair| pair[0] < pair[1])
}

impl ContainerAuthority<'_> {
    /// Enumerate persisted rollouts so a daemon restart resumes the same workflow.
    pub fn rollouts(
        &self,
        instance: InstanceId,
        application: ResourceId,
    ) -> Result<Vec<ContainerRollout>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let mut stmt=conn.prepare("SELECT id FROM container_rollouts WHERE application_id=?1 ORDER BY created_at_ms,id").map_err(|_|invariant())?;
            let ids=stmt.query_map([application],|row|row.get::<_,ResourceId>(0)).map_err(|_|invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|invariant())?;
            ids.into_iter().map(|id|read_rollout(conn,application,id)).collect()
        })
    }
    /// Count the frozen cohort and its positively running target replacements.
    pub fn rollout_progress(
        &self,
        instance: InstanceId,
        application: ResourceId,
        id: ResourceId,
    ) -> Result<(u64, u64), PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let rollout=read_rollout(conn,application,id)?;
            conn.query_row("SELECT COUNT(*),COALESCE(SUM(EXISTS(SELECT 1 FROM container_instance_leases l WHERE l.application_id=?2 AND l.namespace_resource_id=m.namespace_resource_id AND l.object_id=m.object_id AND l.object_generation=m.object_generation AND l.target_generation=?3 AND l.state='running' AND l.stop_requested_at_ms IS NULL)),0) FROM container_rollout_members m WHERE m.rollout_id=?1",params![id,application,rollout.target_generation],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|invariant())
        })
    }
    /// Freeze the current target and old execution cohort before starting a rollout.
    pub fn create_rollout(
        &self,
        instance: InstanceId,
        application: ResourceId,
        request: &ContainerRolloutCreate<'_>,
        now_ms: i64,
    ) -> Result<ContainerRollout, PlatformError> {
        let ContainerRolloutCreate {
            id,
            target_configuration,
            steps,
            details,
        } = *request;
        if target_configuration.is_empty() || target_configuration.len() > 16384 {
            return Err(invariant());
        }
        let expected: serde_json::Value =
            serde_json::from_slice(target_configuration).map_err(|_| invariant())?;
        if !expected.is_object() || !valid_steps(steps) {
            return Err(invariant());
        }
        let default = ContainerRolloutDetails {
            description: None,
            step_descriptions: vec![None; steps.len()],
        };
        let details = details.unwrap_or(&default);
        if details.step_descriptions.len() != steps.len()
            || details
                .description
                .iter()
                .chain(details.step_descriptions.iter().flatten())
                .any(|v| v.len() > 2048 || v.contains('\0'))
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let target: Option<u64> = tx.query_row("SELECT target_generation FROM container_applications WHERE id=?1 AND deletion_requested_at_ms IS NULL AND deleted_at_ms IS NULL", [application], |row| row.get(0)).optional().map_err(|_| invariant())?.flatten();
            let target = target.ok_or_else(not_found)?;
            let configuration: Vec<u8> = tx.query_row(
                "SELECT configuration_json FROM container_target_configuration WHERE application_id=?1 AND target_generation=?2",
                params![application, target], |row| row.get(0),
            ).map_err(|_| invariant())?;
            let current: serde_json::Value =
                serde_json::from_slice(&configuration).map_err(|_| invariant())?;
            if !current.is_object() {
                return Err(invariant());
            }
            if current != expected {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "requested Container configuration is not the current target",
                ));
            }
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM container_rollouts WHERE application_id=?1 AND state='running')",
                [application], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if active {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "Container application already has an active rollout",
                ));
            }
            let steps_json = serde_json::to_vec(steps).map_err(|_| invariant())?;
            let current:u64=tx.query_row("SELECT COALESCE(MIN(target_generation),?2) FROM container_instance_leases WHERE application_id=?1 AND target_generation!=?2 AND state!='stopped'",params![application,target],|r|r.get(0)).map_err(|_|invariant())?;
            let details_json=serde_json::to_vec(details).map_err(|_|invariant())?;
            tx.execute("INSERT INTO container_rollouts(id,application_id,target_generation,steps_json,state,current_step,created_at_ms,updated_at_ms,current_generation,details_json) VALUES(?1,?2,?3,?4,'running',0,?5,?5,?6,?7)", params![id,application,target,steps_json,now_ms,current,details_json]).map_err(|_| invariant())?;
            tx.execute("INSERT INTO container_rollout_members(rollout_id,namespace_resource_id,object_id,object_generation,lease_generation) SELECT ?1,namespace_resource_id,object_id,object_generation,lease_generation FROM container_instance_leases WHERE application_id=?2 AND target_generation!=?3 AND state!='stopped'", params![id,application,target]).map_err(|_| invariant())?;
            read_rollout(tx, application, id)
        })
    }

    /// Read durable rollout state without deriving completion from provider absence.
    pub fn rollout(
        &self,
        instance: InstanceId,
        application: ResourceId,
        id: ResourceId,
    ) -> Result<ContainerRollout, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            read_rollout(conn, application, id)
        })
    }

    /// Reserve exactly the current cumulative cohort and persist stop intent before I/O.
    pub fn rollout_step_leases(
        &self,
        instance: InstanceId,
        application: ResourceId,
        id: ResourceId,
        now_ms: i64,
    ) -> Result<Vec<ContainerLease>, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let rollout = read_rollout(tx, application, id)?;
            if rollout.state != "running" { return Ok(Vec::new()); }
            let count: u64 = tx.query_row("SELECT COUNT(*) FROM container_rollout_members WHERE rollout_id=?1", [id], |row| row.get(0)).map_err(|_| invariant())?;
            let percentage = *rollout.steps.get(rollout.current_step).ok_or_else(invariant)?;
            let limit = count.checked_mul(u64::from(percentage)).ok_or_else(invariant)?.div_ceil(100);
            let mut stmt = tx.prepare("SELECT l.namespace_resource_id,l.object_id,l.object_generation,l.lease_generation FROM container_rollout_members m JOIN container_instance_leases l USING(namespace_resource_id,object_id,object_generation,lease_generation) WHERE m.rollout_id=?1 ORDER BY l.namespace_resource_id,l.object_id,l.object_generation,l.lease_generation LIMIT ?2").map_err(|_| invariant())?;
            let identities = stmt.query_map(params![id,limit], |row| {
                let object: String = row.get(1)?;
                Ok(ContainerLeaseIdentity { namespace_resource_id: row.get(0)?, object_id: object.parse().map_err(|_| rusqlite::Error::InvalidQuery)?, object_generation: row.get(2)?, lease_generation: row.get(3)? })
            }).map_err(|_| invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_| invariant())?;
            let mut leases = Vec::new();
            let mut replacements_ready = true;
            for identity in identities {
                let lease = leases::read_lease(tx, identity)?;
                if lease.state == ContainerLeaseState::Stopped {
                    let ready:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM container_instance_leases WHERE application_id=?1 AND namespace_resource_id=?2 AND object_id=?3 AND object_generation=?4 AND target_generation=?5 AND state='running' AND stop_requested_at_ms IS NULL)",params![application,identity.namespace_resource_id,identity.object_id.to_string(),identity.object_generation,rollout.target_generation],|row|row.get(0)).map_err(|_|invariant())?;
                    replacements_ready &= ready;
                    continue;
                }
                tx.execute("UPDATE container_instance_leases SET state='stopping',stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE opaque_engine_key=?1", params![lease.opaque_engine_key,now_ms]).map_err(|_| invariant())?;
                leases.push(leases::read_lease(tx,identity)?);
            }
            if leases.is_empty() && replacements_ready {
                let next = rollout.current_step.checked_add(1).ok_or_else(invariant)?;
                let state = if next == rollout.steps.len() { "completed" } else { "running" };
                tx.execute("UPDATE container_rollouts SET state=?2,current_step=?3,updated_at_ms=?4 WHERE id=?1", params![id,state,next,now_ms]).map_err(|_| invariant())?;
            }
            Ok(leases)
        })
    }

    /// Return stopped members in the current step that still need a live target execution.
    pub fn rollout_replacements(
        &self,
        instance: InstanceId,
        application: ResourceId,
        id: ResourceId,
    ) -> Result<Vec<ContainerLease>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let rollout = read_rollout(conn, application, id)?;
            if rollout.state != "running" { return Ok(Vec::new()); }
            let count:u64=conn.query_row("SELECT COUNT(*) FROM container_rollout_members WHERE rollout_id=?1",[id],|r|r.get(0)).map_err(|_|invariant())?;
            let limit=count.checked_mul(u64::from(rollout.steps[rollout.current_step])).ok_or_else(invariant)?.div_ceil(100);
            let mut stmt=conn.prepare("SELECT namespace_resource_id,object_id,object_generation,lease_generation FROM container_rollout_members WHERE rollout_id=?1 ORDER BY namespace_resource_id,object_id,object_generation,lease_generation LIMIT ?2").map_err(|_|invariant())?;
            let identities=stmt.query_map(params![id,limit],|r|Ok(ContainerLeaseIdentity{namespace_resource_id:r.get(0)?,object_id:r.get::<_,String>(1)?.parse().map_err(|_|rusqlite::Error::InvalidQuery)?,object_generation:r.get(2)?,lease_generation:r.get(3)?})).map_err(|_|invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|invariant())?;
            let mut pending=Vec::new();
            for identity in identities {
                let lease=leases::read_lease(conn,identity)?;
                if lease.state!=ContainerLeaseState::Stopped {continue;}
                let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM do_objects o JOIN container_applications a ON a.namespace_resource_id=o.namespace_resource_id WHERE a.id=?1 AND a.target_generation=?2 AND a.deletion_requested_at_ms IS NULL AND a.deleted_at_ms IS NULL AND o.namespace_resource_id=?3 AND o.object_id=?4 AND o.generation=?5 AND o.state IN ('creating','ready')) AND NOT EXISTS(SELECT 1 FROM container_instance_leases WHERE application_id=?1 AND namespace_resource_id=?3 AND object_id=?4 AND object_generation=?5 AND target_generation=?2 AND state='running' AND stop_requested_at_ms IS NULL)",params![application,rollout.target_generation,identity.namespace_resource_id,identity.object_id.to_string(),identity.object_generation],|r|r.get(0)).map_err(|_|invariant())?;
                if valid {pending.push(lease);}
            }
            Ok(pending)
        })
    }

    /// Recover frozen startup authority only for a stopped member of the active rollout.
    pub fn replacement_startup(
        &self,
        authority: &ContainerObjectAuthority,
    ) -> Result<Option<ContainerLease>, PlatformError> {
        self.db.with_read(|conn| {
            leases::require_facet_authority(conn,authority)?;
            let id:Option<String>=conn.query_row("SELECT r.id FROM container_rollouts r JOIN container_applications a ON a.id=r.application_id WHERE r.application_id=?1 AND r.state='running' AND r.target_generation=a.target_generation",[authority.application_id],|r|r.get(0)).optional().map_err(|_|invariant())?;
            let Some(id)=id else {return Ok(None);};
            let generation:Option<u64>=conn.query_row("SELECT m.lease_generation FROM container_rollout_members m JOIN container_instance_leases l USING(namespace_resource_id,object_id,object_generation,lease_generation) WHERE m.rollout_id=?1 AND m.namespace_resource_id=?2 AND m.object_id=?3 AND m.object_generation=?4 AND l.state='stopped' ORDER BY m.lease_generation DESC LIMIT 1",params![id,authority.namespace_resource_id,authority.object_id.to_string(),authority.object_generation],|r|r.get(0)).optional().map_err(|_|invariant())?;
            generation.map(|lease_generation| leases::read_lease(conn,ContainerLeaseIdentity{namespace_resource_id:authority.namespace_resource_id,object_id:authority.object_id,object_generation:authority.object_generation,lease_generation})).transpose()
        })
    }

    /// Mark failure while keeping every selected lease and reservation intact.
    pub fn fail_rollout(
        &self,
        instance: InstanceId,
        application: ResourceId,
        id: ResourceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let changed = tx.execute("UPDATE container_rollouts SET state='failed',updated_at_ms=?3 WHERE id=?1 AND application_id=?2 AND state='running'", params![id,application,now_ms]).map_err(|_| invariant())?;
            if changed != 1 { return Err(not_found()); } Ok(())
        })
    }
}

fn read_rollout(
    conn: &Connection,
    application: ResourceId,
    id: ResourceId,
) -> Result<ContainerRollout, PlatformError> {
    let rollout = conn.query_row("SELECT target_generation,steps_json,state,current_step,created_at_ms,updated_at_ms,current_generation,details_json FROM container_rollouts WHERE id=?1 AND application_id=?2", params![id,application], |row| {
        let bytes: Vec<u8> = row.get(1)?;
        Ok(ContainerRollout { id, application_id: application, target_generation: row.get(0)?, steps: serde_json::from_slice(&bytes).map_err(|_| rusqlite::Error::InvalidQuery)?, state: row.get(2)?, current_step: row.get(3)?, created_at_ms: row.get(4)?, updated_at_ms: row.get(5)?,current_generation:row.get(6)?,details:serde_json::from_slice(&row.get::<_,Vec<u8>>(7)?).map_err(|_|rusqlite::Error::InvalidQuery)? })
    }).optional().map_err(|_| invariant())?.ok_or_else(not_found)?;
    if rollout.details.step_descriptions.len() != rollout.steps.len()
        || !valid_steps(&rollout.steps)
        || rollout.current_step > rollout.steps.len()
        || (rollout.state == "running" && rollout.current_step >= rollout.steps.len())
    {
        return Err(invariant());
    }
    Ok(rollout)
}
