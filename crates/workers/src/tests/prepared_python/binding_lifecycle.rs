//! Frozen binding/compatibility inputs and independent restore integrity checks.

use super::*;
use open_compute_core::{CanonicalBindingConfig, CanonicalPermissions};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn binding_and_flags_are_frozen_and_corrupt_authority_cannot_restore() {
    let base =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/prepared-python-run");
    fs::create_dir_all(&base).unwrap();
    let evidence = recovery::Evidence(Some(
        tempfile::Builder::new()
            .prefix("bindings-")
            .tempdir_in(fs::canonicalize(base).unwrap())
            .unwrap(),
    ));
    let config = storage_config(&evidence.0.as_ref().unwrap().path().join("data"));
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let instance = storage.identity().instance_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(instance, "prepared-bindings", RequestId::generate(), 1, 100)
        .unwrap()
        .0
        .id;
    let CreateResourceOutcome::Applied(resource) = ResourceController::new(
        &storage,
        ResourcePins::new(),
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        instance_id: instance,
        kind: BindingKind::KvNamespace,
        name: "cache".to_owned(),
        idempotency_key: "create-cache".to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 10,
    })
    .unwrap() else {
        panic!("first resource create must apply")
    };
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let source = RuntimeSource::new(
        storage.clone(),
        artifacts.clone(),
        BundleLimits::default(),
        python_runtime_pin(),
    )
    .unwrap();
    let preparations = Arc::new(AtomicUsize::new(0));
    let validator = {
        let source = source.clone();
        let storage = storage.clone();
        let artifacts = artifacts.clone();
        let preparations = preparations.clone();
        move |candidate: ValidationCandidate| {
            let source = source.clone();
            let storage = storage.clone();
            let artifacts = artifacts.clone();
            let preparations = preparations.clone();
            async move {
                let key = loader_key(
                    candidate.instance_id,
                    candidate.worker_id,
                    candidate.version_id,
                );
                let digest = hex::encode(candidate.worker_code_sha256);
                let input = source
                    .resolve(&key, &digest, RuntimeScope::Preparation)
                    .await?;
                assert!(input.python_prepared_sha256.is_none());
                let identity = PreparedPythonIdentity::from_snapshot(python_runtime_pin(), &input)?;
                // Opaque authority fixture bytes, not a native workerd snapshot simulation.
                let bytes = SecretBytes::new(format!("prepared:{digest}").into_bytes());
                publish_prepared_python(&storage, &artifacts, &identity, &bytes, 20_000).await?;
                preparations.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    };
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        Arc::new(validator),
        BundleLimits::default(),
    );
    let inputs = [
        (CanonicalPermissions::default(), Vec::new()),
        (
            CanonicalPermissions {
                read: true,
                write: false,
            },
            Vec::new(),
        ),
        (
            CanonicalPermissions {
                read: true,
                write: false,
            },
            vec![
                "python_workers".to_owned(),
                "enable_python_external_sdk".to_owned(),
            ],
        ),
    ];
    let mut versions = Vec::new();
    let mut records = Vec::new();
    let mut inventories = Vec::new();
    let mut binding_digests = Vec::new();
    for (index, (permissions, flags)) in inputs.iter().enumerate() {
        let mut request = version_request(
            instance,
            worker,
            &format!("binding-{index}"),
            "private-token",
        );
        request.deployment_source = None;
        request.runtime_features.compatibility_flags = flags.clone();
        request.bindings.insert(
            "CACHE".to_owned(),
            VersionBindingInput {
                kind: BindingKind::KvNamespace,
                id: resource.resource_id,
                permissions: *permissions,
                config: CanonicalBindingConfig::default(),
            },
        );
        request.content = VersionContent::Worker {
            bundle: CanonicalBundle::build(
                "main.py",
                vec![ModuleInput {
                    name: "main.py".to_owned(),
                    module_type: ModuleType::Python,
                    bytes: b"pass\n".to_vec(),
                }],
                BundleLimits::default(),
            )
            .unwrap()
            .into_bytes()
            .into(),
            assets: None,
        };
        let CreateVersionOutcome::Applied(result) =
            controller.create_version(request.clone()).await.unwrap()
        else {
            panic!("first version upload must apply")
        };
        assert_eq!(result.version.state, VersionState::Ready);
        assert!(matches!(
            controller.create_version(request).await.unwrap(),
            CreateVersionOutcome::Replay(_)
        ));
        assert_eq!(preparations.load(Ordering::SeqCst), index + 1);
        let resolved = source
            .resolve(
                &loader_key(instance, worker, result.version.id),
                &hex::encode(result.version.worker_code_sha256),
                RuntimeScope::Runtime,
            )
            .await
            .unwrap();
        assert_eq!(&resolved.compatibility_flags, flags);
        assert_eq!(resolved.bindings.len(), 1);
        assert_eq!(
            resolved.bindings[0].descriptor.resource_id,
            resource.resource_id
        );
        assert_eq!(resolved.bindings[0].descriptor.permissions, *permissions);
        binding_digests.push(resolved.bindings[0].descriptor_sha256.clone());
        let record = WorkerRepository::new(storage.db())
            .version_snapshot(instance, worker, result.version.id, false)
            .unwrap()
            .python_prepared
            .unwrap();
        let identity: PreparedPythonIdentity =
            serde_json::from_slice(&record.identity_json).unwrap();
        inventories.push(identity.runtime.module_inventory_sha256);
        records.push(record);
        versions.push(result.version);
    }
    assert!(inventories.windows(2).all(|pair| pair[0] == pair[1]));
    assert_ne!(binding_digests[0], binding_digests[1]);
    for index in 1..versions.len() {
        assert_ne!(
            versions[index - 1].worker_code_sha256,
            versions[index].worker_code_sha256
        );
        assert_ne!(
            records[index - 1].prepared_identity_sha256,
            records[index].prepared_identity_sha256
        );
    }
    let active = WorkerRepository::new(storage.db())
        .get_worker(instance, worker)
        .unwrap();
    assert_eq!(
        mock.object_count(),
        4,
        "one source and three encrypted snapshots"
    );
    drop(controller);
    drop(source);
    drop(storage);
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let source = RuntimeSource::new(
        storage.clone(),
        artifacts.clone(),
        BundleLimits::default(),
        python_runtime_pin(),
    )
    .unwrap();
    for (index, version) in versions.iter().enumerate() {
        let key = loader_key(instance, worker, version.id);
        let digest = hex::encode(version.worker_code_sha256);
        let prepared = hex::encode(records[index].prepared_identity_sha256);
        let resolved = source
            .resolve(&key, &digest, RuntimeScope::Runtime)
            .await
            .unwrap();
        assert_eq!(resolved.compatibility_flags, inputs[index].1);
        assert_eq!(resolved.bindings[0].descriptor.permissions, inputs[index].0);
        assert_eq!(
            source
                .resolve_python_prepared(&key, &digest, RuntimeScope::Runtime, &prepared)
                .await
                .unwrap()
                .expose(),
            format!("prepared:{digest}").as_bytes()
        );
    }
    let version = &versions[0];
    let key = loader_key(instance, worker, version.id);
    let digest = hex::encode(version.worker_code_sha256);
    let prepared = hex::encode(records[0].prepared_identity_sha256);
    let faults = [
        (
            "version_immutable_guard",
            "UPDATE worker_versions SET compatibility_flags_json = ?1 WHERE id = ?2",
            serde_json::to_vec(&inputs[0].1).unwrap(),
            serde_json::to_vec(&inputs[2].1).unwrap(),
        ),
        (
            "version_bindings_update_guard",
            "UPDATE version_bindings SET permissions_json = ?1 WHERE version_id = ?2 AND name = 'CACHE'",
            serde_json::to_vec(&inputs[0].0).unwrap(),
            serde_json::to_vec(&inputs[1].0).unwrap(),
        ),
    ];
    let mut connection = rusqlite::Connection::open(config.path.join("control.sqlite")).unwrap();
    for (trigger, sql, original, corrupt) in faults {
        let trigger_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            connection
                .execute(sql, rusqlite::params![&corrupt, version.id])
                .is_err()
        );
        // Simulate persisted corruption only in this owned fixture; reinstate the exact guard atomically.
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
        assert_eq!(
            transaction
                .execute(sql, rusqlite::params![&corrupt, version.id])
                .unwrap(),
            1
        );
        transaction.execute_batch(&trigger_sql).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            source
                .resolve(&key, &digest, RuntimeScope::Runtime)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::VersionInvariantViolation
        );
        assert_eq!(
            source
                .resolve_python_prepared(&key, &digest, RuntimeScope::Runtime, &prepared)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::VersionInvariantViolation
        );
        assert_eq!(
            WorkerRepository::new(storage.db())
                .get_worker(instance, worker)
                .unwrap(),
            active
        );
        assert_eq!(
            WorkerRepository::new(storage.db())
                .version_snapshot(instance, worker, version.id, false)
                .unwrap()
                .python_prepared
                .as_ref(),
            Some(&records[0])
        );
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
        assert_eq!(
            transaction
                .execute(sql, rusqlite::params![&original, version.id])
                .unwrap(),
            1
        );
        transaction.execute_batch(&trigger_sql).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            source
                .resolve_python_prepared(&key, &digest, RuntimeScope::Runtime, &prepared)
                .await
                .unwrap()
                .expose(),
            format!("prepared:{digest}").as_bytes()
        );
    }
    assert_eq!(preparations.load(Ordering::SeqCst), 3);
    assert_eq!(mock.object_count(), 4);
}
