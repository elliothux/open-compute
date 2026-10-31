use super::*;
use crate::durable_objects::{DurableObjectMigrationPlan, DurableObjectMigrationPreparation};
use crate::worker_repository::{VersionContentKind, VersionState};

pub(super) fn stage(
    storage: &PlatformStorage,
    worker: WorkerId,
    declaration: &VersionContainerDeclaration,
    plan: Option<&DurableObjectMigrationPlan>,
    now_ms: i64,
) -> Result<VersionId, PlatformError> {
    let id = VersionId::generate();
    WorkerRepository::new(storage.db()).insert_staging_version(
        &NewVersion {
            id,
            instance_id: storage.identity().instance_id,
            worker_id: worker,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some([7; 32]),
            artifact_size: Some(1),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [8; 32],
            compatibility_date: "2026-09-08".to_owned(),
            compatibility_flags: Vec::new(),
            resource_limits: EffectiveResourceLimits::standard_defaults(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: RequestId::generate(),
            now_ms,
        },
        &NewVersionProducts {
            containers: std::slice::from_ref(declaration),
            durable_object_migration: plan,
            ..NewVersionProducts::default()
        },
        1_000_000,
    )?;
    Ok(id)
}

#[test]
fn container_staging_requires_current_prepared_plan_and_ready_commit_closes_namespaces() {
    let fixture = fixture(2);
    let Fixture {
        directory,
        storage,
        worker,
        ..
    } = fixture;
    let instance = storage.identity().instance_id;
    let repository = DurableObjectRepository::new(&storage);
    let retiring_namespace = repository
        .list_namespaces(instance)
        .unwrap()
        .into_iter()
        .find(|namespace| namespace.class_name == "ContainerClass")
        .unwrap();
    let plan = DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "prepared-container-v1".to_owned(),
        new_sqlite_classes: vec!["PendingContainer".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: vec!["ContainerClass".to_owned()],
    };
    assert_eq!(
        repository
            .prepare_worker_migration(instance, worker, &plan, 30)
            .unwrap(),
        DurableObjectMigrationPreparation::Pending
    );
    let pending = repository
        .namespace_for_worker_upload(instance, worker, "PendingContainer", Some(&plan.new_tag))
        .unwrap();
    let declaration =
        VersionContainerDeclaration::new(pending.resource.id, "pending-app".to_owned()).unwrap();
    let initial_count = storage
        .db()
        .with_read(|conn| {
            Ok(conn
                .query_row("SELECT COUNT(*) FROM worker_versions", [], |r| {
                    r.get::<_, u64>(0)
                })
                .unwrap())
        })
        .unwrap();
    let mut wrong_tag = plan.clone();
    wrong_tag.new_tag = "other-plan".to_owned();
    let mut wrong_class = plan.clone();
    wrong_class.new_sqlite_classes = vec!["OtherContainer".to_owned()];
    let mut stale_base = plan.clone();
    stale_base.old_tag = Some("uncommitted-base".to_owned());
    for invalid in [
        None,
        Some(&wrong_tag),
        Some(&wrong_class),
        Some(&stale_base),
    ] {
        assert!(stage(&storage, worker, &declaration, invalid, 31).is_err());
        assert_eq!(
            storage
                .db()
                .with_read(|conn| Ok(conn
                    .query_row("SELECT COUNT(*) FROM worker_versions", [], |r| r
                        .get::<_, u64>(0))
                    .unwrap()))
                .unwrap(),
            initial_count
        );
    }
    let other = WorkerRepository::new(storage.db())
        .create_worker(
            instance,
            "other-stage-owner",
            RequestId::generate(),
            32,
            100,
        )
        .unwrap()
        .0
        .id;
    assert!(stage(&storage, other, &declaration, Some(&plan), 33).is_err());
    let raw = version(&storage, worker);
    assert!(
        ContainerAuthority::new(storage.db())
            .declare_version(
                instance,
                raw,
                pending.resource.id,
                &declaration.application_name,
                &declaration.descriptor_sha256
            )
            .is_err()
    );
    let retired_declaration =
        VersionContainerDeclaration::new(retiring_namespace.resource.id, "retiring-app".to_owned())
            .unwrap();
    let retiring = stage(&storage, worker, &retired_declaration, Some(&plan), 34).unwrap();
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(retiring).unwrap();
    assert_eq!(
        workers
            .mark_ready_with_durable_object_migration(retiring, worker, &plan, 35)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        workers
            .get_worker_version(instance, worker, retiring)
            .unwrap()
            .state,
        VersionState::Validating
    );
    assert!(
        repository
            .current_worker_migration(worker)
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .list_namespaces(instance)
            .unwrap()
            .iter()
            .any(|namespace| { namespace.resource.id == retiring_namespace.resource.id })
    );
    assert!(
        repository
            .namespace_for_worker_upload(instance, worker, "PendingContainer", Some(&plan.new_tag))
            .is_ok()
    );
    let id = stage(&storage, worker, &declaration, Some(&plan), 36).unwrap();
    let repo = ContainerAuthority::new(storage.db());
    assert_eq!(
        repo.version_declarations(instance, id).unwrap(),
        vec![declaration.clone()]
    );
    let stored_tag: Option<String> = storage
        .db()
        .with_read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT prepared_migration_tag FROM version_containers WHERE version_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap())
        })
        .unwrap();
    assert_eq!(stored_tag.as_deref(), Some(plan.new_tag.as_str()));
    let workers = WorkerRepository::new(storage.db());
    workers.begin_validation(id).unwrap();
    assert_eq!(
        workers.mark_ready(id, 35).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        workers
            .get_worker_version(instance, worker, id)
            .unwrap()
            .state,
        VersionState::Validating
    );
    assert!(
        repository
            .list_namespaces(instance)
            .unwrap()
            .iter()
            .all(|n| n.resource.id != pending.resource.id)
    );
    assert!(
        workers
            .mark_ready_with_durable_object_migration(id, worker, &stale_base, 36)
            .is_err()
    );
    assert_eq!(
        workers
            .get_worker_version(instance, worker, id)
            .unwrap()
            .state,
        VersionState::Validating
    );
    assert!(
        repository
            .current_worker_migration(worker)
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .namespace_for_worker_upload(instance, worker, "PendingContainer", Some(&plan.new_tag))
            .is_ok()
    );
    workers
        .mark_ready_with_durable_object_migration(id, worker, &plan, 37)
        .unwrap();
    assert_eq!(
        workers
            .get_worker_version(instance, worker, id)
            .unwrap()
            .state,
        VersionState::Ready
    );
    assert!(
        repository
            .list_namespaces(instance)
            .unwrap()
            .iter()
            .any(|n| n.resource.id == pending.resource.id)
    );
    assert_eq!(
        repo.version_declarations(instance, id).unwrap(),
        vec![declaration.clone()]
    );
    let active = stage(&storage, worker, &declaration, None, 38).unwrap();
    workers.begin_validation(active).unwrap();
    workers.mark_ready(active, 39).unwrap();
    let active_tag: Option<String> = storage
        .db()
        .with_read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT prepared_migration_tag FROM version_containers WHERE version_id=?1",
                    [active],
                    |row| row.get(0),
                )
                .unwrap())
        })
        .unwrap();
    assert!(active_tag.is_none());
    drop(storage);
    let reopened =
        PlatformStorage::bootstrap(&configuration(&directory.path().join("data")), &SystemClock)
            .unwrap();
    assert_eq!(
        ContainerAuthority::new(reopened.db())
            .version_declarations(instance, id)
            .unwrap(),
        vec![declaration]
    );
    assert_eq!(
        WorkerRepository::new(reopened.db())
            .get_worker_version(instance, worker, id)
            .unwrap()
            .state,
        VersionState::Ready
    );
}

#[test]
fn container_stage_schema_checksum_rejects_drift_without_mutation() {
    let fixture = fixture(2);
    let version = fixture.spec.worker_version_id;
    let before = ContainerAuthority::new(fixture.storage.db())
        .version_declarations(fixture.storage.identity().instance_id, version)
        .unwrap();
    fixture
        .storage
        .db()
        .with_read(|conn| {
            let checksum: String = conn
                .query_row(
                    "SELECT checksum FROM refinery_schema_history WHERE version=16",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let wrong = (checksum.parse::<u64>().unwrap() ^ 1).to_string();
            conn.execute(
                "UPDATE refinery_schema_history SET checksum=?1 WHERE version=16",
                [wrong],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        crate::migrations::inspect_schema(fixture.storage.db())
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        ContainerAuthority::new(fixture.storage.db())
            .version_declarations(fixture.storage.identity().instance_id, version)
            .unwrap(),
        before
    );
}
