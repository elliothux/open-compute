use super::*;
use crate::{NewDeployment, PlatformStorage, WorkerRepository};
use open_compute_core::{RequestId, StorageConfig, WorkflowsConfig, clock::SystemClock};

#[path = "migration_tests.rs"]
mod migration;

#[path = "atomicity_tests.rs"]
mod atomicity_tests;
#[path = "operation_tests.rs"]
mod operation_tests;

fn setup() -> (tempfile::TempDir, PlatformStorage, DeploymentId) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1024 * 1024 * 1024,
        free_space_hard_bytes: 256 * 1024 * 1024,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(
            account,
            "workflow-worker",
            RequestId::generate(),
            0,
            1_000_000,
        )
        .unwrap();
    let deployment = staging(&storage, worker.0.id);
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 1).unwrap();
    (tmp, storage, deployment)
}

fn staging(storage: &PlatformStorage, worker: open_compute_core::WorkerId) -> DeploymentId {
    let id = DeploymentId::generate();
    WorkerRepository::new(storage.db())
        .insert_staging_deployment(
            &NewDeployment {
                id,
                account_id: storage.identity().default_account_id,
                worker_id: worker,
                content_kind: crate::DeploymentContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                compatibility_date: "2026-08-22".into(),
                compatibility_flags: vec![],
                limits: serde_json::json!({"profile":"default"}),
                worker_code_sha256: [2; 32],
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 0,
            },
            &crate::NewDeploymentProducts::default(),
            1_000_000,
        )
        .unwrap();
    id
}

fn ready(storage: &PlatformStorage, deployment: DeploymentId) -> WorkflowDefinition {
    let account = storage.identity().default_account_id;
    let repo = WorkflowRepository::new(storage.db());
    let definition = repo.create_definition(account, "orders", 0).unwrap();
    let version = repo
        .stage_version(account, definition.id, deployment, "Orders", 1)
        .unwrap();
    repo.finish_version(account, version.target.version_id, true, 2)
        .unwrap();
    repo.definition(account, definition.id).unwrap()
}

#[test]
fn workflow_definition_validation_scope_version_freeze_and_retirement() {
    let (_tmp, storage, deployment) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    assert_eq!(
        repo.create_definition(AccountId::generate(), "none", 0)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNotFound
    );
    let definition = ready(&storage, deployment);
    assert_eq!(
        repo.create_definition(account, "orders", 0)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNameConflict
    );
    assert_eq!(
        repo.definition(AccountId::generate(), definition.id)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNotFound
    );
    assert_eq!(
        repo.stage_version(account, definition.id, deployment, "__reserved", 1)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowVersionNotReady
    );
    let before = repo
        .reserve_instance(
            account,
            definition.id,
            Some("first"),
            &WorkflowsConfig::default(),
            3,
        )
        .unwrap();
    repo.rename(account, definition.id, "renamed", 4).unwrap();
    let version2 = repo
        .stage_version(account, definition.id, deployment, "Second", 5)
        .unwrap();
    let version3 = repo
        .stage_version(account, definition.id, deployment, "Third", 6)
        .unwrap();
    repo.finish_version(account, version3.target.version_id, true, 7)
        .unwrap();
    repo.finish_version(account, version2.target.version_id, true, 8)
        .unwrap();
    let after = repo
        .reserve_instance(
            account,
            definition.id,
            Some("second"),
            &WorkflowsConfig::default(),
            9,
        )
        .unwrap();
    assert_eq!(after.identity.target.version_id, version3.target.version_id);
    assert_eq!(
        before.identity.target.version_id,
        definition.current_version_id.unwrap()
    );
    assert_eq!(
        repo.reservation(before.identity.instance_id)
            .unwrap()
            .unwrap()
            .identity
            .target
            .definition_name,
        "orders"
    );
    assert_eq!(after.identity.target.definition_name, "renamed");
    assert_eq!(repo.retire_unused_versions(100, 10).unwrap(), 1);
    assert_eq!(
        repo.delete(account, definition.id, 11).unwrap_err().code(),
        ErrorCode::WorkflowReferenced
    );
    repo.finalize_instance(&before.identity, 12).unwrap();
    repo.retain_instance(&before.identity, 13).unwrap();
    let purge = repo
        .prepare_instance_operation(
            &before.identity,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Purge,
            &WorkflowsConfig::default(),
            13,
        )
        .unwrap();
    repo.complete_instance_operation(&WorkflowAppliedOperation { operation: purge }, 13)
        .unwrap();
    assert!(!repo.instance_referrers_intact(&before.identity).unwrap());
    assert_eq!(repo.retire_unused_versions(100, 14).unwrap(), 1);
    assert_eq!(
        repo.version(account, before.identity.target.version_id)
            .unwrap()
            .state,
        DeploymentState::Tombstoned
    );
    repo.abandon_creation(&after.identity).unwrap();
    repo.delete(account, definition.id, 15).unwrap();
    repo.delete(account, definition.id, 16).unwrap();
    assert_eq!(
        repo.definition(account, definition.id).unwrap().state,
        ResourceState::Tombstoned
    );
    assert_eq!(repo.definitions(account, None, 10).unwrap().len(), 1);
    assert!(
        repo.definitions(account, Some(definition.id), 10)
            .unwrap()
            .is_empty()
    );
    assert!(repo.definitions(account, None, 0).is_err());
}

#[test]
fn workflow_creation_identity_quota_grace_and_referrer_guards() {
    let (_tmp, storage, deployment) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = ready(&storage, deployment);
    let limits = WorkflowsConfig {
        max_instances_per_account: 1,
        max_instances_per_definition: 1,
        max_active_per_account: 1,
        ..Default::default()
    };
    let reservation = repo
        .reserve_instance(account, definition.id, Some("one"), &limits, 3)
        .unwrap();
    let id = &reservation.identity;
    assert_eq!(
        repo.find_instance(definition.id, "one").unwrap().identity,
        *id
    );
    assert_eq!(
        repo.reserve_instance(account, definition.id, Some("one"), &limits, 3)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceAlreadyExists
    );
    assert_eq!(
        repo.reserve_instance(account, definition.id, Some("two"), &limits, 3)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    assert_eq!(
        repo.find_instance(WorkflowId::generate(), "one")
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
    assert!(repo.instance_referrers_intact(id).unwrap());
    storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "DELETE FROM deployment_referrers WHERE kind='workflow_instance' AND ref_id=?1",
                    [id.instance_id.to_string()]
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE workflow_instance_referrers SET version_id=?2 WHERE instance_id=?1",
                    params![
                        id.instance_id.to_string(),
                        WorkflowVersionId::generate().to_string()
                    ]
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(repo.live_reservations(None, 10).unwrap().len(), 1);
    assert!(
        repo.live_reservations(Some(id.instance_id), 10)
            .unwrap()
            .is_empty()
    );
    assert!(repo.abandon_creation(id).unwrap());
    let second = repo
        .reserve_instance(account, definition.id, Some("one"), &limits, 4)
        .unwrap();
    repo.finalize_instance(&second.identity, 5).unwrap();
    repo.finalize_instance(&second.identity, 5).unwrap();
    assert!(!repo.abandon_creation(&second.identity).unwrap());
    repo.repair_instance_referrers(&second.identity).unwrap();
    repo.retain_instance(&second.identity, 6).unwrap();
    let purge = repo
        .prepare_instance_operation(
            &second.identity,
            open_compute_core::WorkflowOperationId::generate(),
            WorkflowOperationKind::Purge,
            &limits,
            6,
        )
        .unwrap();
    repo.complete_instance_operation(&WorkflowAppliedOperation { operation: purge }, 6)
        .unwrap();
    assert!(repo.repair_instance_referrers(&second.identity).is_err());
    assert!(repo.live_reservations(None, 10).unwrap().is_empty());
    repo.mark_unavailable(account, definition.id, 7).unwrap();
    assert_eq!(
        repo.reserve_instance(account, definition.id, None, &WorkflowsConfig::default(), 8)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNotReady
    );
}

#[test]
fn workflow_binding_namespace_hash_and_catalog_reachability() {
    let (_tmp, storage, deployment) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = ready(&storage, deployment);
    let worker = repo
        .version(account, definition.current_version_id.unwrap())
        .unwrap()
        .target
        .worker_id;
    let caller = staging(&storage, worker);
    let binding = repo
        .prepare_binding(account, caller, "ORDERS", definition.id, 1)
        .unwrap();
    assert!(
        repo.prepare_binding(account, caller, "__PRIVATE", definition.id, 1)
            .is_err()
    );
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    assert_eq!(
        storage
            .db()
            .with_read(|conn| bindings::read_workflow_bindings(conn, caller))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repo.delete(account, definition.id, 2).unwrap_err().code(),
        ErrorCode::WorkflowReferenced
    );
    assert!(
        repo.authorize_binding(
            binding.descriptor.binding_id,
            caller,
            &binding.descriptor_sha256
        )
        .is_err()
    );
    storage
        .db()
        .with_immediate(|tx| {
            assert!(
                tx.execute(
                    "INSERT INTO deployment_vars VALUES(?1,'ORDERS',X'6E756C6C')",
                    [caller.to_string()]
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "UPDATE workflow_bindings SET name='OTHER' WHERE id=?1",
                    [binding.descriptor.binding_id.to_string()]
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(caller).unwrap();
    workers.mark_ready(caller, 2).unwrap();
    repo.authorize_binding(
        binding.descriptor.binding_id,
        caller,
        &binding.descriptor_sha256,
    )
    .unwrap();
    assert_eq!(
        repo.authorize_binding(binding.descriptor.binding_id, caller, &[9; 32])
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowBindingStale
    );
    assert_eq!(
        repo.authorize_binding(
            binding.descriptor.binding_id,
            deployment,
            &binding.descriptor_sha256
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowBindingStale
    );
    repo.rename(account, definition.id, "same-binding", 3)
        .unwrap();
    repo.authorize_binding(
        binding.descriptor.binding_id,
        caller,
        &binding.descriptor_sha256,
    )
    .unwrap();
}

#[test]
fn workflow_rejection_does_not_replace_current_version_or_enable_unvalidated_definition() {
    let (_tmp, storage, deployment) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = repo.create_definition(account, "bad", 0).unwrap();
    let version = repo
        .stage_version(account, definition.id, deployment, "Missing", 1)
        .unwrap();
    assert_eq!(
        repo.delete(account, definition.id, 1).unwrap_err().code(),
        ErrorCode::WorkflowReferenced
    );
    repo.finish_version(account, version.target.version_id, false, 2)
        .unwrap();
    assert_eq!(
        repo.reserve_instance(account, definition.id, None, &WorkflowsConfig::default(), 3)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNotReady
    );
    assert!(
        repo.finish_version(account, version.target.version_id, true, 3)
            .is_err()
    );
    assert_eq!(repo.retire_unused_versions(1, 4).unwrap(), 1);
    repo.delete(account, definition.id, 5).unwrap();
}
