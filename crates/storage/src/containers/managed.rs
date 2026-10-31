//! Policy-only targets for Durable Object scheduled applications.

use super::*;

impl ContainerAuthority<'_> {
    /// Create a namespace-identified application without inventing a default image.
    /// The active declaration and operator policy are verified in the same transaction.
    pub fn create_durable_object_application(
        &self,
        instance: InstanceId,
        application: &ContainerApplication,
        spec: &ContainerTargetSpec,
        observability_logs: bool,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if application.scheduling_policy != ContainerSchedulingPolicy::DurableObject
            || application.id != application.namespace_resource_id
            || application.max_instances == 0
            || !valid_application_name(&application.name)
            || spec.image_sha256.is_some()
        {
            return Err(invariant());
        }
        spec.resources.validate(spec.resources)?;
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let current: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM do_namespaces n JOIN resources r ON r.id=n.resource_id AND r.state='ready' JOIN workers w ON w.id=n.owner_worker_id JOIN worker_deployments d ON d.id=w.active_deployment_id JOIN worker_versions v ON v.id=d.version_id AND v.worker_id=w.id JOIN version_containers c ON c.version_id=v.id AND c.namespace_resource_id=n.resource_id WHERE n.resource_id=?1 AND c.application_name=?2 AND v.id=?3 AND n.lifecycle_state='active' AND v.state='ready' AND v.deleted_at_ms IS NULL AND w.deleted_at_ms IS NULL AND d.deleted_at_ms IS NULL)",
                params![application.namespace_resource_id,application.name,spec.worker_version_id], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if !current { return Err(not_found()); }
            let conflict: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM container_applications WHERE deleted_at_ms IS NULL AND (id=?1 OR namespace_resource_id=?1 OR name=?2))",
                params![application.id,application.name], |row| row.get(0),
            ).map_err(|_| invariant())?;
            if conflict { return Err(PlatformError::new(ErrorCode::ResourceNameConflict, "Container application already exists")); }
            tx.execute("INSERT INTO container_applications(id,namespace_resource_id,name,max_instances,scheduling_policy,created_at_ms) VALUES(?1,?1,?2,?3,'durable_object',?4)", params![application.id,application.name,application.max_instances,now_ms]).map_err(|_| invariant())?;
            let resources = serde_json::to_vec(&spec.resources).map_err(|_| invariant())?;
            tx.execute("INSERT INTO container_application_targets(application_id,generation,worker_version_id,image_sha256,provider_contract_sha256,resources_json,enable_internet,created_at_ms) VALUES(?1,1,?2,NULL,?3,?4,?5,?6)",params![application.id,spec.worker_version_id,spec.provider_contract_sha256,resources,spec.enable_internet,now_ms]).map_err(|_|invariant())?;
            tx.execute("INSERT INTO container_target_configuration(application_id,target_generation,configuration_json) VALUES(?1,1,NULL)",params![application.id]).map_err(|_|invariant())?;
            tx.execute("INSERT INTO container_application_settings(application_id,configuration_json,observability_logs,updated_at_ms) VALUES(?1,NULL,?2,?3)",params![application.id,observability_logs,now_ms]).map_err(|_|invariant())?;
            tx.execute("UPDATE container_applications SET target_generation=1,admission_paused=0 WHERE id=?1",[application.id]).map_err(|_|invariant())?;
            Ok(())
        })
    }
}
