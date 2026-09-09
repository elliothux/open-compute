//! Worker v4 domain adapter over the immutable Version/Deployment authority.

use super::model::{WorkerUploadBinding, WorkerUploadExport, WorkerUploadMetadata};
use super::multipart::ParsedWorkerUpload;
use crate::cloudflare_v4::V4ResourceKind;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::workers_http::WorkerApiState;
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, PlatformError,
    RequestId, ResourceId, SecretString, WorkerId,
};
use open_compute_storage::{
    BuiltinBindingKind, CatalogDirection, CatalogSort, DeploymentSource, DurableObjectRepository,
    QueueRepository, ResourceRepository, VersionSnapshot, WorkerRecord, WorkerRepository,
    WorkflowDefinitionReservation, WorkflowRepository,
};
use open_compute_workers::{
    CreateVersionOutcome, CreateVersionRequest, ModuleBindingKind, RuntimeValidator,
    ServiceDescriptorV1, VersionBindingInput, VersionBundle, VersionCachePolicyInput,
    VersionContent, VersionController, VersionModuleBindingInput, VersionRuntimeFeatures,
    VersionServiceInput,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) use super::authority::{ensure_worker, resolve_account, worker_by_name};
pub(super) use super::cloning::clone_active;
use super::errors::{invalid, invariant, unsupported};

#[expect(
    clippy::too_many_arguments,
    reason = "the immutable Version creation boundary keeps authority and audit inputs explicit"
)]
pub(super) async fn create_from_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    worker: &WorkerRecord,
    upload: ParsedWorkerUpload,
    strict_inheritance: bool,
    deployment_source: Option<DeploymentSource>,
    request_id: RequestId,
    now_ms: i64,
) -> Result<CreateVersionOutcome, PlatformError> {
    let migration = super::do_lifecycle::prepare(
        api,
        account_id,
        worker.id,
        &upload.metadata,
        upload.bundle.as_deref(),
        now_ms,
    )?;
    let result = create_from_prepared_upload(
        api,
        account_authority,
        account_id,
        worker,
        upload,
        strict_inheritance,
        deployment_source,
        request_id,
        now_ms,
        migration.as_ref(),
    )
    .await;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            if let Some(migration) = &migration {
                migration.rollback(api, worker.id, now_ms)?;
            }
            Err(error)
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
async fn create_from_prepared_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    worker: &WorkerRecord,
    upload: ParsedWorkerUpload,
    strict_inheritance: bool,
    deployment_source: Option<DeploymentSource>,
    request_id: RequestId,
    now_ms: i64,
    migration: Option<&super::do_lifecycle::PreparedDoMigration>,
) -> Result<CreateVersionOutcome, PlatformError> {
    let mut input = UploadInput::new(upload.metadata);
    let previous = worker
        .active_version_id
        .map(|version| {
            WorkerRepository::new(api.storage.db())
                .version_snapshot(account_id, worker.id, version, false)
        })
        .transpose()?;
    input.apply_inheritance(api, previous.as_ref(), strict_inheritance)?;
    let reservation_owner = request_id.to_string();
    if let Err(error) = input.apply_explicit_bindings(
        api,
        account_authority,
        account_id,
        worker.id,
        migration.map(super::do_lifecycle::PreparedDoMigration::tag),
        false,
        true,
        Some(&reservation_owner),
        now_ms,
    ) {
        input.release_workflow_reservations(api, account_id, now_ms)?;
        return Err(error);
    }
    let (content, asset_session) = match input
        .content(
            api,
            account_id,
            &worker.name,
            upload.bundle,
            Some(&reservation_owner),
            now_ms,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => {
            input.release_workflow_reservations(api, account_id, now_ms)?;
            return Err(error);
        }
    };
    let idempotency_key = match &asset_session {
        Some(reservation) => format!(
            "v4-assets/{}",
            reservation.operation_id.as_deref().ok_or_else(invariant)?
        ),
        None => format!("v4/{request_id}"),
    };
    let validator: Arc<dyn RuntimeValidator> = Arc::new(api.transport.clone());
    let mut controller = VersionController::new(
        &api.storage,
        api.artifacts.clone(),
        validator,
        api.bundle_limits,
    )
    .with_queue_consumer_limit(api.max_queue_consumer_concurrency);
    if let Some(promoter) = &api.product_promoter {
        controller = controller.with_product_promoter(promoter.clone());
    }
    if let Some(migration) = migration {
        controller = controller.with_durable_object_migration(migration.plan().clone());
    }
    let workflow_reservations = std::mem::take(&mut input.workflow_reservations);
    let outcome = controller
        .create_version(CreateVersionRequest {
            account_id,
            worker_id: worker.id,
            idempotency_key,
            content,
            vars: input.vars,
            secrets: input.secrets,
            bindings: input.bindings,
            services: input.services,
            runtime_features: input.runtime_features,
            queue_consumers: Vec::new(),
            crons: input.crons,
            deployment_source,
            request_id,
            now_ms,
        })
        .await;
    match outcome {
        Ok(outcome) => {
            if let Some(session) = asset_session {
                super::assets::consume_assets(api, &session, now_ms)?;
            }
            Ok(outcome)
        }
        Err(error) => {
            if let Some(session) = asset_session
                && error.code() != ErrorCode::IdempotencyConflict
            {
                super::assets::release_assets(api, &session, now_ms)?;
            }
            release_workflow_reservations(api, account_id, &workflow_reservations, now_ms)?;
            Err(error)
        }
    }
}

pub(super) async fn validate_new_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    script_name: &str,
    upload: &ParsedWorkerUpload,
    strict_inheritance: bool,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let mut input = UploadInput::new(upload.metadata.clone());
    input.apply_inheritance(api, None, strict_inheritance)?;
    input.apply_explicit_bindings(
        api,
        account_authority,
        account_id,
        WorkerId::generate(),
        None,
        true,
        false,
        None,
        now_ms,
    )?;
    input
        .content(
            api,
            account_id,
            script_name,
            upload.bundle.clone(),
            None,
            now_ms,
        )
        .await?;
    Ok(())
}

mod upload;

pub(super) use upload::UploadInput;

fn release_workflow_reservations(
    api: &WorkerApiState,
    account: AccountId,
    reservations: &[WorkflowDefinitionReservation],
    now_ms: i64,
) -> Result<(), PlatformError> {
    let repository = WorkflowRepository::new(api.storage.db());
    for reservation in reservations {
        repository.release_definition_reservation(account, reservation, now_ms)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;
