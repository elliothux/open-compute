//! Cloudflare Worker binding response projection from immutable Version snapshots.

use crate::cloudflare_v4::V4ResourceKind;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::workers_http::WorkerApiState;
use open_compute_core::{BindingKind, ErrorCode, PlatformError};
use open_compute_storage::{
    BuiltinBindingKind, QueueRepository, ResourceRepository, VersionSnapshot, WorkerRepository,
    WorkflowRepository,
};
use open_compute_workers::ServiceDescriptorV1;

pub(super) fn public_bindings(
    api: &WorkerApiState,
    authority: &AccountAuthority,
    snapshot: &VersionSnapshot,
) -> Result<Vec<serde_json::Value>, PlatformError> {
    let mut values = Vec::new();
    for (name, bytes) in &snapshot.vars {
        let value = serde_json::from_slice::<serde_json::Value>(bytes).map_err(|_| invariant())?;
        if let Some(text) = value.as_str() {
            values.push(serde_json::json!({"name": name, "type": "plain_text", "text": text}));
        } else {
            values.push(serde_json::json!({"name": name, "type": "json", "json": value}));
        }
    }
    values.extend(
        snapshot
            .secrets
            .keys()
            .map(|name| serde_json::json!({"name": name, "type": "secret_text"})),
    );
    let resources = ResourceRepository::new(api.storage.db());
    for binding in &snapshot.bindings {
        let resource = resources.get(snapshot.account_id, binding.resource_id)?;
        let value = match binding.kind {
            BindingKind::KvNamespace => serde_json::json!({
                "name": binding.name,
                "type": "kv_namespace",
                "namespace_id": authority.public_resource_id(V4ResourceKind::KvNamespace, resource.id),
            }),
            BindingKind::D1Database => serde_json::json!({
                "name": binding.name,
                "type": "d1",
                "id": authority.public_resource_id(V4ResourceKind::D1Database, resource.id),
            }),
            BindingKind::DoNamespace => serde_json::json!({
                "name": binding.name,
                "type": "durable_object_namespace",
                "class_name": resource.name,
                "namespace_id": authority.public_resource_id(V4ResourceKind::DurableObjectNamespace, resource.id),
            }),
            BindingKind::R2Bucket => {
                named_binding(&binding.name, "r2_bucket", "bucket_name", &resource.name)
            }
            BindingKind::VectorizeIndex => {
                named_binding(&binding.name, "vectorize", "index_name", &resource.name)
            }
            BindingKind::AiSearchNamespace => named_binding(
                &binding.name,
                "ai_search_namespace",
                "namespace",
                &resource.name,
            ),
            BindingKind::AiSearchInstance => {
                named_binding(&binding.name, "ai_search", "instance_name", &resource.name)
            }
            BindingKind::QueueProducer | BindingKind::Workflow => return Err(invariant()),
        };
        values.push(value);
    }
    let queues = QueueRepository::new(api.storage.db());
    for binding in &snapshot.queue_bindings {
        let queue = queues.get(snapshot.account_id, binding.queue_id)?;
        values.push(serde_json::json!({
            "name": binding.name,
            "type": "queue",
            "queue_name": queue.name,
        }));
    }
    let workflows = WorkflowRepository::new(api.storage.db());
    for binding in &snapshot.workflow_bindings {
        let definition =
            workflows.definition(snapshot.account_id, binding.descriptor.definition_id)?;
        values.push(serde_json::json!({
            "name": binding.descriptor.name,
            "type": "workflow",
            "workflow_name": definition.name,
        }));
    }
    let workers = WorkerRepository::new(api.storage.db()).list_workers(snapshot.account_id)?;
    for binding in &snapshot.services {
        let props = binding
            .props_json
            .as_deref()
            .map(serde_json::from_slice::<serde_json::Value>)
            .transpose()
            .map_err(|_| invariant())?;
        let descriptor = ServiceDescriptorV1::new(
            binding.binding_name.clone(),
            binding.target_worker_id,
            binding.entrypoint.clone(),
            props,
        )
        .map_err(|_| invariant())?;
        let canonical_props = descriptor
            .props
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| invariant())?;
        if canonical_props != binding.props_json
            || descriptor.sha256().map_err(|_| invariant())? != binding.descriptor_sha256
        {
            return Err(invariant());
        }
        let service = workers
            .iter()
            .find(|worker| worker.id == binding.target_worker_id)
            .ok_or_else(invariant)?;
        let mut value = serde_json::json!({
            "name": binding.binding_name,
            "type": "service",
            "service": service.name,
            "entrypoint": binding.entrypoint,
        });
        if let Some(props) = descriptor.props {
            value
                .as_object_mut()
                .ok_or_else(invariant)?
                .insert("props".to_owned(), props);
        }
        values.push(value);
    }
    values.extend(snapshot.builtin_bindings.iter().map(|binding| {
        let kind = match binding.kind {
            BuiltinBindingKind::Ai => "ai",
            BuiltinBindingKind::Images => "images",
            BuiltinBindingKind::VersionMetadata => "version_metadata",
            BuiltinBindingKind::WasmModule => "wasm_module",
            BuiltinBindingKind::TextBlob => "text_blob",
            BuiltinBindingKind::DataBlob => "data_blob",
        };
        serde_json::json!({"name": binding.name, "type": kind})
    }));
    Ok(values)
}

pub(super) fn wrangler_kind(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::KvNamespace => "kv_namespace",
        BindingKind::R2Bucket => "r2_bucket",
        BindingKind::D1Database => "d1",
        BindingKind::DoNamespace => "durable_object_namespace",
        BindingKind::VectorizeIndex => "vectorize",
        BindingKind::AiSearchNamespace => "ai_search_namespace",
        BindingKind::AiSearchInstance => "ai_search",
        BindingKind::QueueProducer => "queue",
        BindingKind::Workflow => "workflow",
    }
}

fn named_binding(name: &str, kind: &str, identifier: &str, value: &str) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    object.insert(
        "type".to_owned(),
        serde_json::Value::String(kind.to_owned()),
    );
    object.insert(
        identifier.to_owned(),
        serde_json::Value::String(value.to_owned()),
    );
    serde_json::Value::Object(object)
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted Version authority is inconsistent",
    )
}
