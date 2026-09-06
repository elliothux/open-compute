use super::*;

#[tokio::test]
async fn version_pipeline_uploads_validates_promotes_and_replays() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "pipeline", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let (target, _) = repo
        .create_worker(
            account,
            "pipeline-target",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(AcceptAllValidator);
    let controller = VersionController::new(
        &storage,
        artifact_store(&mock),
        validator,
        BundleLimits::default(),
    );
    let mut request = version_request(account, worker.id, "deploy-key", "pipeline-secret-value");
    request.runtime_features.worker_loaders = vec!["LOADER".to_owned()];
    request.services.insert(
        "CATALOG".to_owned(),
        VersionServiceInput {
            target_worker_id: target.id,
            entrypoint: Some("CatalogApi".to_owned()),
            props: Some(serde_json::json!({
                "constructor": {"enabled": true},
                "z": [1, {"__proto__": "ordinary JSON data"}],
            })),
        },
    );
    request.runtime_features.cache.entrypoints.insert(
        "CachedApi".to_owned(),
        VersionCachePolicyInput {
            enabled: true,
            cross_version_cache: false,
        },
    );
    let first = controller.create_version(request.clone()).await.unwrap();
    let (version_id, descriptor_hash) = match first {
        CreateVersionOutcome::Applied(result) => {
            assert!(result.deployment.is_some());
            assert_eq!(result.version.state, VersionState::Ready);
            (
                result.version.id,
                hex::encode(result.version.worker_code_sha256),
            )
        }
        CreateVersionOutcome::Replay(_) => panic!("first request cannot replay"),
    };
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(version_id)
    );
    assert_eq!(mock.object_count(), 1);
    let replay = controller.create_version(request.clone()).await.unwrap();
    match replay {
        CreateVersionOutcome::Replay(bytes) => {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains(&version_id.to_string()));
            assert!(!text.contains("pipeline-secret-value"));
        }
        CreateVersionOutcome::Applied(_) => panic!("idempotency replay created a version"),
    }
    assert_eq!(repo.list_versions(account, worker.id).unwrap().len(), 1);
    assert_eq!(mock.object_count(), 1);

    let source = RuntimeSource::new(
        storage.clone(),
        artifact_store(&mock),
        BundleLimits::default(),
    );
    let snapshot = source
        .resolve(
            &loader_key(account, worker.id, version_id),
            &descriptor_hash,
            RuntimeScope::Runtime,
        )
        .await
        .unwrap();
    assert!(format!("{source:?}").contains("RuntimeSource"));
    assert!(format!("{snapshot:?}").contains("RuntimeSnapshot"));
    assert!(format!("{:?}", snapshot.modules[0]).contains("RuntimeModule"));
    assert_eq!(snapshot.modules.len(), 1);
    assert_eq!(snapshot.vars["MODE"], "production");
    let observability = snapshot.observability.as_ref().unwrap();
    assert_eq!(observability.account_id, account.to_string());
    assert_eq!(observability.worker_id, worker.id.to_string());
    assert_eq!(observability.version_id, version_id.to_string());
    assert_eq!(observability.script_name, worker.name);
    assert_eq!(observability.observability_generation, 1);
    assert!(observability.enabled && observability.logs_enabled && observability.persist);
    assert_eq!(snapshot.services.len(), 1);
    assert_eq!(snapshot.services[0].descriptor.name, "CATALOG");
    assert_eq!(snapshot.services[0].descriptor.target_worker_id, target.id);
    assert_eq!(
        snapshot.services[0].descriptor.entrypoint.as_deref(),
        Some("CatalogApi")
    );
    assert_eq!(
        snapshot.services[0].descriptor.props,
        request.services["CATALOG"].props
    );
    assert_eq!(
        snapshot.secrets["API_TOKEN"].expose(),
        "pipeline-secret-value"
    );
    assert!(!format!("{snapshot:?}").contains("pipeline-secret-value"));
    let namespace = worker_loader_namespace_key(account, worker.id, "LOADER");
    assert_eq!(
        snapshot.worker_loaders,
        vec![RuntimeWorkerLoaderBinding {
            name: "LOADER".to_owned(),
            namespace_key: namespace.clone(),
        }]
    );
    assert_eq!(
        worker_loader_namespaces(storage.db(), account, worker.id).unwrap(),
        vec![namespace.clone()]
    );
    assert!(worker_loader_namespaces(storage.db(), AccountId::generate(), worker.id).is_err());
    assert!(!format!("{snapshot:?}").contains(&namespace));
    assert!(!format!("{:?}", snapshot.worker_loaders).contains(&namespace));
    let payload = RuntimeSource::internal_payload(&snapshot).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(payload.expose()).unwrap();
    assert_eq!(
        wire["workerLoaders"],
        serde_json::json!([{ "name": "LOADER", "namespaceKey": namespace }])
    );
    assert!(
        std::str::from_utf8(payload.expose())
            .unwrap()
            .contains("pipeline-secret-value")
    );
    assert!(
        std::str::from_utf8(payload.expose())
            .unwrap()
            .contains("observabilityGeneration")
    );
    assert!(!format!("{payload:?}").contains("pipeline-secret-value"));
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                "bad-descriptor",
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );

    let cache = Arc::new(
        ArtifactCache::open(
            root.join("artifact-cache-test"),
            CacheConfig {
                max_bytes: 64 * 1024 * 1024,
                high_watermark_ratio: 0.9,
                low_watermark_ratio: 0.5,
                partial_grace_ms: 50,
                max_artifact_bytes: 32 * 1024 * 1024,
            },
            StartupId::generate(),
        )
        .unwrap(),
    );
    let cached_source = RuntimeSource::new(
        storage.clone(),
        artifact_store(&mock),
        BundleLimits::default(),
    )
    .with_cache(cache);
    let probe = cached_source
        .resolve(
            &loader_key(account, worker.id, version_id),
            &descriptor_hash,
            RuntimeScope::Probe,
        )
        .await
        .unwrap();
    assert!(probe.secrets.is_empty());
    assert!(probe.observability.is_none());
    assert_eq!(probe.modules, snapshot.modules);
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Validation,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );

    let canonical_service_props =
        serde_json::to_vec(request.services["CATALOG"].props.as_ref().unwrap()).unwrap();
    let mut conflict = request;
    conflict.secrets.insert(
        "API_TOKEN".to_owned(),
        SecretString::new("different-secret"),
    );
    assert_eq!(
        controller
            .create_version(conflict)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(repo.version_referrers(version_id).unwrap().len(), 1);
    assert_eq!(
        repo.prune_expired_idempotency(10_000 + 24 * 60 * 60 * 1_000 + 1, 64)
            .unwrap(),
        1
    );
    assert!(repo.version_referrers(version_id).unwrap().is_empty());

    for entry in fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = fs::read(path).unwrap();
            assert!(
                !bytes
                    .windows(b"pipeline-secret-value".len())
                    .any(|window| { window == b"pipeline-secret-value" })
            );
        }
    }

    // Simulate out-of-band corruption by removing production guards. The
    // RuntimeSource descriptor checks must still fail closed.
    let conn = rusqlite::Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute_batch("DROP TRIGGER version_services_update_guard;")
        .unwrap();
    conn.execute(
        "UPDATE version_services SET props_json = ?1 WHERE version_id = ?2 AND binding_name = 'CATALOG'",
        rusqlite::params![
            br#"{"z":[1,{"__proto__":"ordinary JSON data"}],"constructor":{"enabled":true}}"#,
            version_id.to_string()
        ],
    )
    .unwrap();
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
    conn.execute(
        "UPDATE version_services SET props_json = ?1 WHERE version_id = ?2 AND binding_name = 'CATALOG'",
        rusqlite::params![
            canonical_service_props,
            version_id.to_string()
        ],
    )
    .unwrap();
    conn.execute_batch("DROP TRIGGER version_immutable_guard;")
        .unwrap();
    conn.execute(
        "UPDATE worker_versions SET worker_code_sha256 = zeroblob(32) WHERE id = ?1",
        [version_id.to_string()],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
}
