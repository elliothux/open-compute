//! Persistent Container control workflows over an independently qualified provider.

use crate::container_broker::{BrokerLease, QualifiedContainerProvider};
use open_compute_artifacts::{ArtifactCache, ArtifactStore};
use open_compute_core::{
    DurableObjectId, ErrorCode, InstanceId, PlatformError, ResourceId,
    containers::ContainerLeaseState,
};
use open_compute_storage::{
    PlatformStorage,
    containers::{
        ContainerAuthority, ContainerLeaseIdentity, ContainerObjectAuthority,
        ContainerRunningObservation, ContainerTermination, VersionContainerIdentity,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

mod control;
mod credentials;
mod directories;
mod registration;
mod rollouts;
mod snapshots;
pub(crate) use snapshots::SnapshotRequest;
mod startup;
pub(crate) mod wire;

/// One installation's Container authority; construction requires active Broker qualification.
pub struct ContainerService {
    pub(crate) storage: Arc<PlatformStorage>,
    pub(crate) artifacts: ArtifactStore,
    pub(crate) cache: Arc<ArtifactCache>,
    pub(crate) provider: QualifiedContainerProvider,
    credential_rotation: tokio::sync::Mutex<std::collections::HashSet<String>>,
    snapshot_captures: std::sync::Mutex<std::collections::HashSet<ResourceId>>,
}

impl std::fmt::Debug for ContainerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerService")
            .field("instance_id", &self.instance_id())
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

impl ContainerService {
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        cache: Arc<ArtifactCache>,
        provider: QualifiedContainerProvider,
    ) -> Self {
        Self {
            storage,
            artifacts,
            cache,
            provider,
            credential_rotation: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            snapshot_captures: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Check the already qualified live Broker without repeating resource qualification.
    pub(crate) async fn ensure_active(&self) -> Result<(), PlatformError> {
        self.provider
            .ensure_active()
            .await
            .map_err(|_| unavailable())
    }

    /// Maximum streamed registry request permitted by the qualified image contract.
    pub(crate) fn registry_request_bytes(&self) -> u64 {
        self.provider.max_image_bytes()
    }

    pub(crate) fn registry_upload_path(
        &self,
        id: ResourceId,
    ) -> Result<std::path::PathBuf, PlatformError> {
        Ok(self
            .storage
            .data_dir()
            .prepare_tmp_dir()?
            .join(format!("container-registry-{id}.part")))
    }

    pub(crate) fn instance_id(&self) -> InstanceId {
        self.storage.identity().instance_id
    }

    /// Resolve only trusted physical-facet authority; start reserves capacity before provider I/O.
    pub(crate) async fn resolve(
        &self,
        body: ResolveRequest,
        now_ms: i64,
        generation: &str,
    ) -> Result<ResolveResponse, PlatformError> {
        if body.instance_id != self.instance_id()
            || (body.for_monitor && (body.start || body.for_stop || body.startup.is_some()))
            || (body.start && body.for_stop)
        {
            return Err(unavailable());
        }
        let descriptor = digest(&body.descriptor_sha256)?;
        let repo = ContainerAuthority::new(self.storage.db());
        let app = repo.resolve_declared_application(
            body.instance_id,
            &VersionContainerIdentity {
                namespace_resource_id: body.namespace_resource_id,
                application_name: body.application_name.clone(),
                descriptor_sha256: descriptor,
            },
            body.object_id,
            body.object_generation,
            body.route_generation,
        )?;
        let Some(app) = app else {
            return if body.start {
                Err(unavailable())
            } else {
                Ok(ResolveResponse { lease: None })
            };
        };
        let authority = ContainerObjectAuthority {
            instance_id: body.instance_id,
            application_id: app.id,
            namespace_resource_id: body.namespace_resource_id,
            object_id: body.object_id,
            object_generation: body.object_generation,
            route_generation: body.route_generation,
            descriptor_sha256: descriptor,
        };
        let engine_key = repo
            .existing_lease(&authority)?
            .map_or_else(opaque_key, |lease| lease.opaque_engine_key);
        let execution = if body.start {
            Some(self.startup_execution(&body, &app, &authority, &engine_key, now_ms)?)
        } else {
            None
        };
        let lease = if body.start {
            repo.reserve(
                &authority,
                &engine_key,
                execution.as_ref().ok_or_else(unavailable)?,
                self.provider.max_instances(),
                now_ms,
            )
            .map(Some)?
        } else {
            repo.existing_lease(&authority)?
        };
        let Some(lease) = lease else {
            return Ok(ResolveResponse { lease: None });
        };
        if body.start {
            let envelope = execution
                .as_ref()
                .map(|spec| &spec.startup_envelope)
                .ok_or_else(unavailable)?;
            let requested = self.open_start_options(
                envelope,
                &lease.identity.owner_key(body.instance_id),
                &engine_key,
            )?;
            if requested != self.start_options(&lease)? {
                return Err(unavailable());
            }
        }
        if body.for_monitor
            && !matches!(
                lease.state,
                ContainerLeaseState::Starting
                    | ContainerLeaseState::Running
                    | ContainerLeaseState::Lost
            )
        {
            return Err(unavailable());
        }
        if lease.stop_requested_at_ms.is_some() && !body.for_stop {
            return Err(unavailable());
        }
        if body.for_stop {
            let request = StopRequest {
                instance_id: body.instance_id,
                application_name: body.application_name,
                route_generation: body.route_generation,
                descriptor_sha256: body.descriptor_sha256,
                namespace_resource_id: body.namespace_resource_id,
                object_id: body.object_id,
                object_generation: body.object_generation,
                lease_generation: lease.identity.lease_generation,
                opaque_engine_key: lease.opaque_engine_key,
            };
            self.stop(&request, now_ms).await?;
            return Ok(ResolveResponse { lease: None });
        }
        let target = repo.target(body.instance_id, app.id, lease.target_generation)?;
        if target.spec.provider_contract_sha256 != digest(self.provider.contract_sha256())? {
            return Err(unavailable());
        }
        lease
            .effective_resources
            .validate(self.provider.resources())?;
        let image = repo.image(body.instance_id, &lease.effective_image_sha256)?;
        let image_ref = format!("sha256:{}", hex::encode(image.config_sha256));
        if body.start && lease.stop_requested_at_ms.is_none() {
            self.materialize_image(&lease.effective_image_sha256)
                .await?;
            let registration = self.registration(&lease)?;
            if lease.state == ContainerLeaseState::Admitting {
                repo.mark_starting(lease.identity, &lease.opaque_engine_key, now_ms)?;
            }
            if self
                .provider
                .register(&registration)
                .await
                .inspect_err(|error| {
                    tracing::warn!(
                        phase = "container-registration",
                        error_code = error.code().as_str(),
                        operator_message = error.message(),
                        "Container admission failed"
                    );
                })
                .is_err()
                || self
                    .prepare_directories(&authority, &lease, &registration)
                    .await
                    .inspect_err(|error| {
                        tracing::warn!(
                            phase = "container-directory-preparation",
                            error_code = error.code().as_str(),
                            operator_message = error.message(),
                            "Container admission failed"
                        );
                    })
                    .is_err()
            {
                repo.mark_lost(lease.identity, &lease.opaque_engine_key, now_ms)?;
                return Err(unavailable());
            }
        }
        if !body.start
            && !body.for_monitor
            && lease.stop_requested_at_ms.is_none()
            && matches!(
                lease.state,
                ContainerLeaseState::Starting
                    | ContainerLeaseState::Running
                    | ContainerLeaseState::Lost
            )
        {
            let observed = self.provider.observe(&lease.opaque_engine_key).await;
            match observed {
                Ok(observed)
                    if observed.running
                        && observed.image == image_ref
                        && observed.manifest_sha256
                            == hex::encode(lease.effective_image_sha256)
                        && observed.provider_contract_sha256 == self.provider.contract_sha256() =>
                {
                    digest(&observed.start_identity)?;
                    let commit = repo.observe_running(
                        lease.identity,
                        &ContainerRunningObservation {
                            opaque_engine_key: &lease.opaque_engine_key,
                            start_identity: &observed.start_identity,
                            image_sha256: &lease.effective_image_sha256,
                            provider_contract_sha256: &target.spec.provider_contract_sha256,
                        },
                        now_ms,
                    );
                    if commit.is_err() {
                        repo.mark_lost(lease.identity, &lease.opaque_engine_key, now_ms)?;
                        return Err(unavailable());
                    }
                }
                _ => {
                    repo.mark_lost(lease.identity, &lease.opaque_engine_key, now_ms)?;
                }
            }
        }
        let lease = repo.lease(body.instance_id, lease.identity)?;
        let credentials = self
            .credentials_for_generation(&lease.opaque_engine_key, generation)
            .await?;
        let options = self.start_options(&lease)?;
        let inspection = ContainerInspection {
            image: lease.startup.image_reference.clone(),
            labels: options.labels.clone(),
        };
        let mut native_startup = serde_json::json!({"enableInternet":options.enable_internet,"environmentVariables":options.environment_variables,"labels":options.labels});
        if let Some(entrypoint) = options.entrypoint {
            native_startup["entrypoint"] = serde_json::json!(entrypoint);
        }
        if lease.startup.image_override {
            native_startup["source"] = serde_json::json!({"image":format!("startup@sha256:{}",hex::encode(lease.effective_image_sha256))});
        }
        if let Some(name) = &lease.startup.instance_profile {
            native_startup["instance"] = serde_json::json!({"named":name.as_str()});
        } else if lease.startup.resources_override {
            native_startup["instance"] = serde_json::json!({"custom":{"vcpu":lease.effective_resources.cpu_nanos as f64 / 1_000_000_000.0,"memoryMib":(lease.effective_resources.memory_bytes / (1<<20)).to_string(),"diskMb":(lease.effective_resources.disk_bytes / 1_000_000).to_string()}});
        }
        let native_startup_json =
            serde_json::to_string(&native_startup).map_err(|_| unavailable())?;
        Ok(ResolveResponse {
            lease: Some(ResolvedLease {
                lease_generation: lease.identity.lease_generation,
                target_generation: lease.target_generation,
                opaque_engine_key: lease.opaque_engine_key,
                start_identity: lease.start_identity,
                state: lease.state,
                stop_requested_at_ms: lease.stop_requested_at_ms,
                image_ref,
                image_sha256: hex::encode(lease.effective_image_sha256),
                provider_contract_sha256: hex::encode(target.spec.provider_contract_sha256),
                resources: lease.effective_resources,
                enable_internet: lease.enable_internet,
                control_token: credentials.control_token,
                callback_token: credentials.callback_token,
                native_startup_json,
                inspection,
            }),
        })
    }

    /// Persist termination intent, then require the Broker's positive cleanup and tombstone.
    pub(crate) fn begin_stop(
        &self,
        request: &StopRequest,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repo = ContainerAuthority::new(self.storage.db());
        let lease = repo.lease(self.instance_id(), request.identity())?;
        if lease.opaque_engine_key != request.opaque_engine_key {
            return Err(unavailable());
        }
        let authority = self.request_authority(request)?;
        repo.begin_authorized_stop(&authority, lease.identity, &lease.opaque_engine_key, now_ms)
    }

    fn request_authority(
        &self,
        request: &StopRequest,
    ) -> Result<ContainerObjectAuthority, PlatformError> {
        if request.instance_id != self.instance_id() {
            return Err(unavailable());
        }
        let descriptor = digest(&request.descriptor_sha256)?;
        let app = ContainerAuthority::new(self.storage.db())
            .resolve_declared_application(
                request.instance_id,
                &VersionContainerIdentity {
                    namespace_resource_id: request.namespace_resource_id,
                    application_name: request.application_name.clone(),
                    descriptor_sha256: descriptor,
                },
                request.object_id,
                request.object_generation,
                request.route_generation,
            )?
            .ok_or_else(unavailable)?;
        Ok(ContainerObjectAuthority {
            instance_id: request.instance_id,
            application_id: app.id,
            namespace_resource_id: request.namespace_resource_id,
            object_id: request.object_id,
            object_generation: request.object_generation,
            route_generation: request.route_generation,
            descriptor_sha256: descriptor,
        })
    }

    /// Require positive Broker cleanup before releasing a persisted execution.
    pub(crate) async fn stop(
        &self,
        request: &StopRequest,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repo = ContainerAuthority::new(self.storage.db());
        self.begin_stop(request, now_ms)?;
        let lease = repo.lease(self.instance_id(), request.identity())?;
        if lease.state == ContainerLeaseState::Stopped {
            return Ok(());
        }
        if self.terminate_lease(&lease, None).await.is_err() {
            repo.mark_lost(lease.identity, &lease.opaque_engine_key, now_ms)?;
            return Err(unavailable());
        }
        repo.finish_stopped(
            lease.identity,
            &ContainerTermination {
                opaque_engine_key: &lease.opaque_engine_key,
                start_identity: lease.start_identity.as_deref(),
                terminated: true,
                cleanup_complete: true,
                late_operations_fenced: true,
            },
            now_ms,
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolveRequest {
    pub instance_id: InstanceId,
    pub namespace_resource_id: ResourceId,
    pub application_name: String,
    pub object_id: DurableObjectId,
    pub object_generation: u64,
    pub route_generation: u64,
    pub descriptor_sha256: String,
    pub start: bool,
    pub for_stop: bool,
    pub for_monitor: bool,
    pub startup: Option<startup::Startup>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveResponse {
    pub lease: Option<ResolvedLease>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedLease {
    pub lease_generation: u64,
    pub target_generation: u64,
    pub opaque_engine_key: String,
    pub start_identity: Option<String>,
    pub state: ContainerLeaseState,
    pub stop_requested_at_ms: Option<i64>,
    pub image_ref: String,
    pub image_sha256: String,
    pub provider_contract_sha256: String,
    pub resources: open_compute_core::containers::ContainerResources,
    pub enable_internet: bool,
    pub control_token: String,
    pub callback_token: String,
    pub native_startup_json: String,
    pub inspection: ContainerInspection,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContainerInspection {
    pub image: String,
    pub labels: Vec<crate::container_broker::BrokerLabel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StopRequest {
    pub instance_id: InstanceId,
    pub application_name: String,
    pub route_generation: u64,
    pub descriptor_sha256: String,
    pub namespace_resource_id: ResourceId,
    pub object_id: DurableObjectId,
    pub object_generation: u64,
    pub lease_generation: u64,
    pub opaque_engine_key: String,
}
impl StopRequest {
    fn identity(&self) -> ContainerLeaseIdentity {
        ContainerLeaseIdentity {
            namespace_resource_id: self.namespace_resource_id,
            object_id: self.object_id,
            object_generation: self.object_generation,
            lease_generation: self.lease_generation,
        }
    }
}

pub(crate) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "Container provider is unavailable",
    )
}
fn digest(value: &str) -> Result<[u8; 32], PlatformError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(unavailable());
    }
    hex::decode(value)
        .map_err(|_| unavailable())?
        .try_into()
        .map_err(|_| unavailable())
}
fn opaque_key() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}
fn expected_start_identity(key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"open-compute-container-start-v1\0");
    hash.update(key.as_bytes());
    hex::encode(hash.finalize())
}

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod named_images_tests;
#[cfg(test)]
mod rollouts_tests;

#[cfg(test)]
mod materialization_tests;

#[cfg(test)]
mod snapshot_tests;
