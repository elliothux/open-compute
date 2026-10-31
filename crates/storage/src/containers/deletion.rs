use super::*;

impl ContainerAuthority<'_> {
    /// Fence application admission and all its executions before any deletion I/O.
    pub fn begin_application_delete(
        &self,
        instance: InstanceId,
        application: ResourceId,
        now: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed=tx.execute("UPDATE container_applications SET admission_paused=1,deletion_requested_at_ms=COALESCE(deletion_requested_at_ms,?2) WHERE id=?1 AND deleted_at_ms IS NULL",params![application,now]).map_err(|_|invariant())?;
            if changed!=1 {return Err(not_found());}
            tx.execute("UPDATE container_instance_leases SET state='stopping',stop_requested_at_ms=COALESCE(stop_requested_at_ms,?2),updated_at_ms=?2 WHERE application_id=?1 AND state!='stopped'",params![application,now]).map_err(|_|invariant())?;
            tx.execute("UPDATE container_snapshots SET state='retiring' WHERE application_id=?1",[application]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Enumerate deletion intents so daemon restart can resume positive cleanup.
    pub fn pending_application_deletions(
        &self,
        instance: InstanceId,
    ) -> Result<Vec<ResourceId>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let mut stmt=conn.prepare("SELECT id FROM container_applications WHERE deletion_requested_at_ms IS NOT NULL AND deleted_at_ms IS NULL ORDER BY id").map_err(|_|invariant())?;
            stmt.query_map([],|row| row.get(0)).map_err(|_|invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|invariant())
        })
    }

    /// Retire the public identity only after every retained execution is positively terminal.
    pub fn finish_application_delete(
        &self,
        instance: InstanceId,
        application: ResourceId,
        now: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed=tx.execute("UPDATE container_applications SET deleted_at_ms=?2 WHERE id=?1 AND deletion_requested_at_ms IS NOT NULL AND deleted_at_ms IS NULL AND NOT EXISTS(SELECT 1 FROM container_instance_leases WHERE application_id=?1 AND state!='stopped') AND NOT EXISTS(SELECT 1 FROM container_snapshots WHERE application_id=?1)",params![application,now]).map_err(|_|invariant())?;
            if changed!=1 {return Err(invariant());} Ok(())
        })
    }
}
