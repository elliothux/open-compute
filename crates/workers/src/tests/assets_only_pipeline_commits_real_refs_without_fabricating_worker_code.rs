use super::*;

#[tokio::test]
async fn assets_only_pipeline_commits_real_refs_without_fabricating_worker_code() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "static-site", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let mock = MockS3::spawn("open-compute").await;
    let store = artifact_store(&mock);
    let bytes = bytes::Bytes::from_static(b"hello assets");
    let digest = sha2::Sha256::digest(&bytes);
    store
        .put_verified(
            futures::stream::once(async { Ok::<_, std::io::Error>(bytes) }),
            &hex::encode(digest),
            12,
        )
        .await
        .unwrap();
    let assets = VersionAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries: vec![AssetEntryV1 {
                path: "/index.html".to_owned(),
                sha256: hex::encode(digest),
                size: 12,
                content_type: "text/html; charset=utf-8".to_owned(),
            }],
        },
        routing: AssetRoutingConfigV1 {
            schema_version: 1,
            binding: None,
            run_worker_first: RunWorkerFirst::All(false),
            html_handling: HtmlHandling::AutoTrailingSlash,
            not_found_handling: NotFoundHandling::Page404,
            headers: Vec::new(),
            redirects: Vec::new(),
        },
    };
    let controller = VersionController::new(
        &storage,
        store.clone(),
        Arc::new(AcceptAllValidator),
        BundleLimits::default(),
    );
    let request = CreateVersionRequest {
        account_id: account,
        worker_id: worker.id,
        idempotency_key: "assets-only".to_owned(),
        content: VersionContent::AssetsOnly {
            assets: assets.clone(),
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: 10,
    };
    let result = match controller.create_version(request.clone()).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result,
        CreateVersionOutcome::Replay(_) => panic!("first assets version replayed"),
    };
    assert_eq!(
        result.version.content_kind,
        open_compute_storage::VersionContentKind::AssetsOnly
    );
    assert!(result.version.artifact_sha256.is_none());
    assert!(result.version.main_module.is_none());
    let stored = open_compute_storage::VersionAssetsRepository::new(storage.db())
        .get(result.version.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.logical_file_count, 1);
    assert_eq!(stored.logical_total_bytes, 12);
    assert_eq!(workers.referenced_artifacts().unwrap().len(), 2);
    assert_eq!(mock.object_count(), 2);
    let static_snapshot = RuntimeSource::new(storage.clone(), store, BundleLimits::default())
        .resolve(
            &loader_key(account, worker.id, result.version.id),
            &hex::encode(result.version.worker_code_sha256),
            RuntimeScope::Runtime,
        )
        .await
        .unwrap();
    assert_eq!(static_snapshot.main_module, None);
    assert!(static_snapshot.modules.is_empty());
    assert!(static_snapshot.assets.is_some());

    let mut invalid = request;
    invalid.idempotency_key = "assets-only-env".to_owned();
    invalid
        .vars
        .insert("MODE".to_owned(), serde_json::json!("x"));
    assert_eq!(
        controller.create_version(invalid).await.unwrap_err().code(),
        ErrorCode::AssetConfigUnsupported
    );
}
