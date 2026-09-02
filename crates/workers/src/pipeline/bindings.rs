//! Prepare immutable product binding descriptors from their owning authorities.
use super::*;

pub(super) struct PreparedBindings {
    pub(super) descriptors: Vec<BindingDescriptorV1>,
    pub(super) rows: Vec<NewVersionBinding>,
    pub(super) queue_descriptors: Vec<QueueProducerBindingDescriptorV1>,
    pub(super) queue_rows: Vec<NewQueueProducerBinding>,
    pub(super) workflow_descriptors: Vec<open_compute_storage::WorkflowBindingDescriptor>,
    pub(super) workflow_rows: Vec<open_compute_storage::WorkflowBindingRecord>,
    pub(super) durable_object_classes: Vec<String>,
    pub(super) service_descriptors: Vec<ServiceDescriptorV1>,
    pub(super) service_rows: Vec<NewVersionService>,
}

impl VersionController<'_> {
    pub(super) fn prepare_bindings(
        &self,
        request: &CreateVersionRequest,
        version: VersionId,
    ) -> Result<PreparedBindings, PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        let queues = QueueRepository::new(self.storage.db());
        let mut descriptors = Vec::with_capacity(request.bindings.len());
        let mut rows = Vec::with_capacity(request.bindings.len());
        let mut queue_descriptors = Vec::new();
        let mut queue_rows = Vec::new();
        let mut workflow_descriptors = Vec::new();
        let mut workflow_rows = Vec::new();
        let mut durable_object_classes = Vec::new();
        let mut service_descriptors = Vec::with_capacity(request.services.len());
        let mut service_rows = Vec::with_capacity(request.services.len());
        for (name, input) in &request.bindings {
            if input.kind == BindingKind::Workflow {
                if input.permissions != CanonicalPermissions::default() {
                    return Err(PlatformError::new(
                        ErrorCode::WorkflowBindingStale,
                        "Workflow binding does not accept resource permissions",
                    ));
                }
                let definition = open_compute_core::WorkflowId::from_uuid(input.id.as_uuid())
                    .map_err(|_| invariant())?;
                let binding = open_compute_storage::WorkflowRepository::new(self.storage.db())
                    .prepare_binding(
                        request.account_id,
                        version,
                        name,
                        definition,
                        input.config.workflow_schedules.clone(),
                        request.now_ms,
                    )?;
                workflow_descriptors.push(binding.descriptor.clone());
                workflow_rows.push(binding);
                continue;
            }
            if input.kind == BindingKind::QueueProducer {
                if input.permissions != CanonicalPermissions::default()
                    || input.config != CanonicalBindingConfig::default()
                {
                    return Err(PlatformError::new(
                        ErrorCode::BindingTypeMismatch,
                        "Queue producer binding does not accept resource permissions or config",
                    ));
                }
                let queue_id = QueueId::from_uuid(input.id.as_uuid()).map_err(|_| invariant())?;
                let queue = queues.get(request.account_id, queue_id)?;
                if queue.state != QueueState::Ready
                    || queue.availability != QueueAvailability::Healthy
                {
                    return Err(PlatformError::new(
                        ErrorCode::QueueNotReady,
                        "version Queue binding is not ready",
                    ));
                }
                let descriptor = QueueProducerBindingDescriptorV1::new(
                    BindingId::generate(),
                    name.clone(),
                    queue.id,
                    queue.lifecycle_generation,
                    1,
                )?;
                queue_rows.push(NewQueueProducerBinding {
                    id: descriptor.binding_id,
                    name: descriptor.name.clone(),
                    queue_id: descriptor.queue_id,
                    queue_lifecycle_generation: descriptor.queue_lifecycle_generation,
                    capability_version: descriptor.capability_version,
                    descriptor_sha256: descriptor.sha256()?,
                });
                queue_descriptors.push(descriptor);
                continue;
            }
            let resource = repository.get(request.account_id, input.id)?;
            if resource.state != ResourceState::Ready {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "version binding resource is not ready",
                ));
            }
            if resource.kind != input.kind {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotFound,
                    "resource was not found in the requested scope",
                ));
            }
            if input.kind == BindingKind::DoNamespace {
                let namespace = DurableObjectRepository::new(self.storage)
                    .get_namespace(request.account_id, input.id)?;
                if namespace.owner_worker_id != request.worker_id {
                    return Err(PlatformError::new(
                        ErrorCode::DoNamespaceNotFound,
                        "Durable Object namespace is not owned by this Worker",
                    ));
                }
                durable_object_classes.push(namespace.class_name);
            }
            let descriptor = BindingDescriptorV1::new(
                BindingId::generate(),
                name.clone(),
                input.kind,
                input.id,
                resource.spec_generation,
                1,
                input.permissions,
                input.config.clone(),
            )?;
            let permissions_json =
                serde_json::to_vec(&descriptor.permissions).map_err(|_| invariant())?;
            let config_json = serde_json::to_vec(&descriptor.config).map_err(|_| invariant())?;
            rows.push(NewVersionBinding {
                id: descriptor.binding_id,
                name: descriptor.name.clone(),
                kind: descriptor.kind,
                resource_id: descriptor.resource_id,
                resource_spec_generation: descriptor.resource_spec_generation,
                capability_version: descriptor.capability_version,
                permissions_json,
                config_json,
                descriptor_sha256: descriptor.sha256()?,
            });
            descriptors.push(descriptor);
        }
        durable_object_classes.sort();
        durable_object_classes.dedup();
        for (name, input) in &request.services {
            WorkerRepository::new(self.storage.db())
                .get_worker(request.account_id, input.target_worker_id)
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::ServiceBindingDenied,
                        "Service target is outside the caller authority",
                    )
                })?;
            let descriptor = ServiceDescriptorV1::new(
                name.clone(),
                input.target_worker_id,
                input.entrypoint.clone(),
            )?;
            service_rows.push(NewVersionService {
                binding_name: descriptor.name.clone(),
                target_worker_id: descriptor.target_worker_id,
                entrypoint: descriptor.entrypoint.clone(),
                descriptor_sha256: descriptor.sha256()?,
            });
            service_descriptors.push(descriptor);
        }
        Ok(PreparedBindings {
            descriptors,
            rows,
            queue_descriptors,
            queue_rows,
            durable_object_classes,
            workflow_descriptors,
            workflow_rows,
            service_descriptors,
            service_rows,
        })
    }
}
