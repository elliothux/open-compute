use super::*;
use applications::insert_image;
use leases::{read_lease, require_facet_authority};
use management::{read_image, require_ordinary_image};

mod persistence;
use persistence::{read_snapshot, snapshots_conn};

/// Snapshot retention after capture or the first positive observation of a restore.
pub const CONTAINER_SNAPSHOT_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

impl ContainerAuthority<'_> {
    /// Reserve snapshot capacity and freeze its exact running source before provider I/O.
    pub fn reserve_snapshot(
        &self,
        authority: &ContainerObjectAuthority,
        input: &NewContainerSnapshot,
        limits: ContainerSnapshotLimits,
        now_ms: i64,
    ) -> Result<ContainerSnapshot, PlatformError> {
        if input.reserved_bytes == 0
            || input.reserved_bytes > i64::MAX as u64
            || limits.max_snapshots == 0
            || limits.max_bytes == 0
            || input
                .name
                .as_ref()
                .is_some_and(|name| name.len() > 256 || name.contains('\0'))
        {
            return Err(invariant());
        }
        let expires = expiry(now_ms)?;
        self.db.with_immediate(|tx| {
            require_facet_authority(tx, authority)?;
            let lease = require_source(tx, authority.application_id, &input.source, input.directory.is_some())?;
            if lease.identity.namespace_resource_id != authority.namespace_resource_id
                || lease.identity.object_id != authority.object_id
                || lease.identity.object_generation != authority.object_generation { return Err(invariant()); }
            let (count, bytes):(u64,u64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(CASE WHEN state!='ready' OR artifact_size IS NULL THEN reserved_bytes ELSE artifact_size END),0) FROM container_snapshots",[],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|invariant())?;
            if count>=u64::from(limits.max_snapshots) || bytes.checked_add(input.reserved_bytes).is_none_or(|total|total>limits.max_bytes) {
                return Err(PlatformError::new(ErrorCode::AdmissionBusy,"Container snapshot retention budget is exhausted"));
            }
            let source_image=if let Some(id)=lease.startup.snapshot_id {
                read_snapshot(tx,id)?.source_image_sha256
            } else { lease.effective_image_sha256 };
            let identity = input.source.identity;
            tx.execute("INSERT INTO container_snapshots (id,instance_id,application_id,namespace_resource_id,object_id,object_generation,lease_generation,opaque_engine_key,start_identity,source_image_sha256,name,state,reserved_bytes,created_at_ms,expires_at_ms,directory_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',?12,?13,?14,?15)",params![input.id,authority.instance_id,authority.application_id,identity.namespace_resource_id,identity.object_id.to_string(),identity.object_generation,identity.lease_generation,input.source.opaque_engine_key,input.source.start_identity,source_image,input.name,input.reserved_bytes,now_ms,expires,input.directory.as_ref().map(open_compute_core::containers::ContainerDirectoryPath::as_str)]).map_err(|_|invariant())?;
            read_snapshot(tx,input.id)
        })
    }

    /// Persist immutable artifact identity before PUT; unknown publication keeps its reservation.
    pub fn stage_snapshot_artifact(
        &self,
        instance: InstanceId,
        id: ResourceId,
        source: &ContainerSnapshotSource,
        image: &ContainerImage,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let snapshot=read_snapshot(tx,id)?;
            if snapshot.instance_id!=instance || snapshot.source!=*source || snapshot.state!=ContainerSnapshotState::Pending
                || image.artifact_size>snapshot.reserved_bytes { return Err(invariant()); }
            require_source(tx,snapshot.application_id,source,snapshot.directory.is_some())?;
            let original=read_image(tx,&snapshot.source_image_sha256)?;
            if original.architecture!=image.architecture {return Err(invariant());}
            insert_image(tx,image,true)?;
            tx.execute("UPDATE container_snapshots SET image_sha256=?2,artifact_sha256=?3,artifact_size=?4 WHERE id=?1",params![id,image.manifest_sha256,image.artifact_sha256,image.artifact_size]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Record an external PUT before sending it; an unfinished attempt retains its byte charge.
    pub fn mark_snapshot_put_attempted(
        &self,
        instance: InstanceId,
        id: ResourceId,
        source: &ContainerSnapshotSource,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let snapshot = read_snapshot(tx, id)?;
            if snapshot.instance_id != instance
                || snapshot.source != *source
                || snapshot.state != ContainerSnapshotState::Pending
                || snapshot.image_sha256.is_none()
                || snapshot.artifact_sha256.is_none()
                || snapshot.artifact_size.is_none()
            {
                return Err(invariant());
            }
            require_source(
                tx,
                snapshot.application_id,
                source,
                snapshot.directory.is_some(),
            )?;
            tx.execute(
                "UPDATE container_snapshots SET put_attempted=1 WHERE id=?1",
                [id],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Publish a verified artifact and freeze its provider image size under live capture authority.
    pub fn finish_snapshot_capture(
        &self,
        authority: &ContainerObjectAuthority,
        id: ResourceId,
        source: &ContainerSnapshotSource,
        image_size: u64,
        now_ms: i64,
    ) -> Result<ContainerSnapshot, PlatformError> {
        let image_size = i64::try_from(image_size).map_err(|_| invariant())?;
        let expires = expiry(now_ms)?;
        self.db.with_immediate(|tx| {
            require_facet_authority(tx,authority)?;
            let snapshot=read_snapshot(tx,id)?;
            if snapshot.instance_id!=authority.instance_id || snapshot.application_id!=authority.application_id
                || snapshot.source!=*source || snapshot.state!=ContainerSnapshotState::Pending
                || snapshot.image_sha256.is_none() || snapshot.expires_at_ms<=now_ms
                || source.identity.namespace_resource_id!=authority.namespace_resource_id
                || source.identity.object_id!=authority.object_id
                || source.identity.object_generation!=authority.object_generation {return Err(invariant());}
            require_source(tx,snapshot.application_id,source,snapshot.directory.is_some())?;
            tx.execute("UPDATE container_snapshots SET state='ready',captured_at_ms=?2,expires_at_ms=?3,image_size=?4 WHERE id=?1",params![id,now_ms,expires,image_size]).map_err(|_|invariant())?;
            read_snapshot(tx,id)
        })
    }

    /// Resolve an application-owned snapshot; reservation repeats this check atomically with capacity.
    pub fn snapshot_for_restore(
        &self,
        authority: &ContainerObjectAuthority,
        id: ResourceId,
        now_ms: i64,
    ) -> Result<ContainerImage, PlatformError> {
        self.db.with_read(|conn| {
            require_facet_authority(conn, authority)?;
            restore_image(conn, authority, id, now_ms)
        })
    }

    /// Resolve a ready directory snapshot under the same application ACL as whole snapshots.
    /// Its image carries directory bytes only and must never be used as a Container boot image.
    pub fn directory_snapshot_for_restore(
        &self,
        authority: &ContainerObjectAuthority,
        id: ResourceId,
        now_ms: i64,
    ) -> Result<(ContainerSnapshot, ContainerImage), PlatformError> {
        self.db.with_read(|conn| {
            require_facet_authority(conn, authority)?;
            let (snapshot, image) = restore_snapshot(conn, authority, id, now_ms)?;
            if snapshot.directory.is_none() {
                return Err(not_found());
            }
            Ok((snapshot, image))
        })
    }

    /// Enumerate durable capture and retirement work without granting tenant access.
    pub fn snapshots(&self, instance: InstanceId) -> Result<Vec<ContainerSnapshot>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            snapshots_conn(conn)
        })
    }

    /// Fence expired snapshots and snapshots of deleting applications before artifact GC.
    pub fn retire_snapshots(
        &self,
        instance: InstanceId,
        now_ms: i64,
    ) -> Result<usize, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            tx.execute("UPDATE container_snapshots SET state='retiring' WHERE state!='retiring' AND ((expires_at_ms<=?1 AND NOT EXISTS(SELECT 1 FROM container_instance_leases l WHERE l.state!='stopped' AND (l.snapshot_id=container_snapshots.id OR EXISTS(SELECT 1 FROM json_each(l.directory_restores_json) d WHERE json_extract(d.value,'$.snapshotId')=container_snapshots.id)))) OR application_id IN (SELECT id FROM container_applications WHERE deletion_requested_at_ms IS NOT NULL))",[now_ms]).map_err(|_|invariant())
        })
    }

    /// Retire a still-pending capture; a concurrent publication wins without stopping its source.
    pub fn retire_snapshot(
        &self,
        instance: InstanceId,
        id: ResourceId,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            read_snapshot(tx,id)?;
            let changed=tx.execute("UPDATE container_snapshots SET state='retiring' WHERE id=?1 AND state='pending'",[id]).map_err(|_|invariant())?;
            Ok(changed==1)
        })
    }

    /// Release retained bytes after positive absence, except unfinished external PUT attempts.
    /// All restore executions must be terminal; an unknown PUT permanently retains its charge.
    pub fn finish_snapshot_retirement(
        &self,
        instance: InstanceId,
        id: ResourceId,
        artifact_absent: bool,
    ) -> Result<(), PlatformError> {
        if !artifact_absent {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let snapshot=read_snapshot(tx,id)?;
            let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM container_instance_leases l WHERE l.state!='stopped' AND (l.snapshot_id=?1 OR EXISTS(SELECT 1 FROM json_each(l.directory_restores_json) d WHERE json_extract(d.value,'$.snapshotId')=?1)))",[id],|row|row.get(0)).map_err(|_|invariant())?;
            // A source that may still finish an unknown capture cannot release its reservation.
            if snapshot.state!=ContainerSnapshotState::Retiring || live {return Err(invariant());}
            if snapshot.captured_at_ms.is_none() && (snapshot.put_attempted
                || read_lease(tx,snapshot.source.identity)?.state!=ContainerLeaseState::Stopped) {return Err(invariant());}
            tx.execute("DELETE FROM container_snapshots WHERE id=?1",[id]).map_err(|_|invariant())?;
            Ok(())
        })
    }
}

fn expiry(now_ms: i64) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(CONTAINER_SNAPSHOT_RETENTION_MS)
        .ok_or_else(invariant)
}

fn require_snapshot_policy(
    conn: &Connection,
    application: ResourceId,
    directory: bool,
) -> Result<(), PlatformError> {
    let allowed: bool = conn.query_row("SELECT (?2 OR scheduling_policy='durable_object') FROM container_applications WHERE id=?1 AND deletion_requested_at_ms IS NULL AND deleted_at_ms IS NULL", params![application, directory], |row| row.get(0)).optional().map_err(|_| invariant())?.unwrap_or(false);
    if !allowed {
        return Err(not_found());
    }
    Ok(())
}

fn require_source(
    conn: &Connection,
    application: ResourceId,
    source: &ContainerSnapshotSource,
    directory: bool,
) -> Result<ContainerLease, PlatformError> {
    require_snapshot_policy(conn, application, directory)?;
    let lease = read_lease(conn, source.identity)?;
    if lease.application_id != application
        || lease.opaque_engine_key != source.opaque_engine_key
        || lease.start_identity.as_deref() != Some(source.start_identity.as_str())
        || lease.state != ContainerLeaseState::Running
        || lease.stop_requested_at_ms.is_some()
        || !valid_opaque(&source.start_identity)
    {
        return Err(invariant());
    }
    Ok(lease)
}

pub(super) fn restore_image(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
    id: ResourceId,
    now_ms: i64,
) -> Result<ContainerImage, PlatformError> {
    let (snapshot, image) = restore_snapshot(conn, authority, id, now_ms)?;
    if snapshot.directory.is_some() {
        return Err(not_found());
    }
    Ok(image)
}

pub(super) fn restore_snapshot(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
    id: ResourceId,
    now_ms: i64,
) -> Result<(ContainerSnapshot, ContainerImage), PlatformError> {
    let result = retained_snapshot(conn, authority, id)?;
    if result.0.expires_at_ms <= now_ms {
        return Err(not_found());
    }
    Ok(result)
}

pub(super) fn retained_snapshot(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
    id: ResourceId,
) -> Result<(ContainerSnapshot, ContainerImage), PlatformError> {
    let snapshot = read_snapshot(conn, id)?;
    require_snapshot_policy(conn, authority.application_id, snapshot.directory.is_some())?;
    if snapshot.instance_id != authority.instance_id
        || snapshot.application_id != authority.application_id
        || snapshot.state != ContainerSnapshotState::Ready
    {
        return Err(not_found());
    }
    let image = retained_image(conn, &snapshot)?;
    Ok((snapshot, image))
}

fn retained_image(
    conn: &Connection,
    snapshot: &ContainerSnapshot,
) -> Result<ContainerImage, PlatformError> {
    let image = read_image(conn, &snapshot.image_sha256.ok_or_else(invariant)?)?;
    let source = read_image(conn, &snapshot.source_image_sha256)?;
    if image.artifact_sha256 != snapshot.artifact_sha256.ok_or_else(invariant)?
        || image.artifact_size != snapshot.artifact_size.ok_or_else(invariant)?
        || image.architecture != source.architecture
    {
        return Err(invariant());
    }
    Ok(image)
}

pub(super) fn retained_directory_image(
    conn: &Connection,
    instance: InstanceId,
    application: ResourceId,
    id: ResourceId,
) -> Result<ContainerImage, PlatformError> {
    let snapshot = read_snapshot(conn, id)?;
    if snapshot.instance_id != instance
        || snapshot.application_id != application
        || snapshot.directory.is_none()
        || !matches!(
            snapshot.state,
            ContainerSnapshotState::Ready | ContainerSnapshotState::Retiring
        )
    {
        return Err(not_found());
    }
    let image = retained_image(conn, &snapshot)?;
    if image.diff_ids.len() != 1
        || image.unpacked_bytes == 0
        || snapshot.image_size != Some(image.unpacked_bytes)
    {
        return Err(invariant());
    }
    Ok(image)
}

pub(super) fn validate_execution(
    conn: &Connection,
    authority: &ContainerObjectAuthority,
    execution: &ContainerExecutionSpec,
    now_ms: i64,
) -> Result<(), PlatformError> {
    if let Some(id) = execution.startup.snapshot_id {
        if execution.startup.image_override {
            return Err(invariant());
        }
        let image = restore_image(conn, authority, id, now_ms)?;
        if image.manifest_sha256 != execution.image_sha256 {
            return Err(invariant());
        }
    } else {
        require_ordinary_image(conn, &execution.image_sha256)?;
    }
    Ok(())
}

pub(super) fn renew_restore(
    conn: &Connection,
    lease: &ContainerLease,
    now_ms: i64,
) -> Result<(), PlatformError> {
    if lease.start_identity.is_some() {
        return Ok(());
    }
    let ids: std::collections::BTreeSet<String> = lease
        .startup
        .snapshot_id
        .into_iter()
        .chain(
            lease
                .directory_snapshots
                .iter()
                .filter_map(|entry| entry.snapshot_id),
        )
        .map(|id| id.to_string())
        .collect();
    for id in ids {
        let until = expiry(now_ms)?;
        let changed=conn.execute("UPDATE container_snapshots SET last_restored_at_ms=MAX(COALESCE(last_restored_at_ms,?2),?2),expires_at_ms=MAX(expires_at_ms,?3) WHERE id=?1 AND application_id=?4",params![id,now_ms,until,lease.application_id]).map_err(|_|invariant())?;
        if changed != 1 {
            return Err(invariant());
        }
    }
    Ok(())
}
