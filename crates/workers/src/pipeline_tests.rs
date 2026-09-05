use super::*;
use crate::assets::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, HtmlHandling, NotFoundHandling,
};
use crate::{ModuleInput, ModuleType};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::{DataConfig, PlatformConfig, SystemClock};

fn version_assets(binding: Option<&str>, worker_first: RunWorkerFirst) -> VersionAssets {
    VersionAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries: vec![AssetEntryV1 {
                path: "/index.html".to_owned(),
                sha256: hex::encode([1; 32]),
                size: 1,
                content_type: "text/html".to_owned(),
            }],
        },
        routing: AssetRoutingConfigV1 {
            schema_version: 1,
            binding: binding.map(str::to_owned),
            run_worker_first: worker_first,
            html_handling: HtmlHandling::AutoTrailingSlash,
            not_found_handling: NotFoundHandling::None,
            headers: Vec::new(),
            redirects: Vec::new(),
        },
    }
}

fn assets_only_request(assets: &VersionAssets) -> CreateVersionRequest {
    CreateVersionRequest {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        idempotency_key: "asset-test".to_owned(),
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
        deployment_source: None,
        request_id: RequestId::generate(),
        now_ms: 1,
    }
}

fn worker_request(
    account_id: AccountId,
    worker_id: WorkerId,
    source: &[u8],
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: "migration-replay".to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: None,
        request_id: RequestId::generate(),
        now_ms: 1,
    }
}

#[tokio::test]
async fn default_entrypoint_validation_and_internal_error_are_stable() {
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_: ValidationCandidate| async { Ok(()) });
    let candidate = ValidationCandidate {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        worker_code_sha256: [3; 32],
    };
    assert_eq!(
        validator
            .validate_entrypoint(candidate.clone(), "named".to_owned())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::EntrypointNotFound
    );
    assert_eq!(invariant().code(), ErrorCode::VersionInvariantViolation);
    assert_eq!(
        validator
            .validate_durable_object_class(candidate, "Counter".to_owned())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DoClassNotFound
    );
}

#[test]
fn assets_only_content_rejects_execution_and_binding_collisions() {
    let assets = version_assets(Some("ASSETS"), RunWorkerFirst::All(false));
    let content = PreparedContent::AssetsOnly {
        assets: assets.clone(),
    };
    let request = assets_only_request(&assets);
    assert!(validate_asset_content(&request, &content, &BTreeMap::new()).is_ok());
    assert_eq!(content.kind(), VersionContentKind::AssetsOnly);
    assert!(content.bundle().is_none());
    assert_eq!(content.assets(), Some(&assets));
    assert!(content.admission_bytes().unwrap() > 64 * 1024);

    let mut vars = BTreeMap::new();
    vars.insert("ASSETS".to_owned(), serde_json::json!(true));
    assert_eq!(
        validate_asset_content(&request, &content, &vars)
            .unwrap_err()
            .code(),
        ErrorCode::BindingTypeMismatch
    );

    let mut request = assets_only_request(&version_assets(None, RunWorkerFirst::All(false)));
    let content = PreparedContent::AssetsOnly {
        assets: version_assets(None, RunWorkerFirst::All(false)),
    };
    request
        .secrets
        .insert("SECRET".to_owned(), SecretString::new("value"));
    assert_eq!(
        validate_asset_content(&request, &content, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::AssetConfigUnsupported
    );

    let request = assets_only_request(&version_assets(None, RunWorkerFirst::All(true)));
    let content = PreparedContent::AssetsOnly {
        assets: version_assets(None, RunWorkerFirst::All(true)),
    };
    assert_eq!(
        validate_asset_content(&request, &content, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::AssetConfigUnsupported
    );
}

#[test]
fn asset_store_errors_are_sanitized_by_product_semantics() {
    for (input, expected) in [
        (
            ErrorCode::ArtifactIntegrityError,
            ErrorCode::AssetIntegrityError,
        ),
        (ErrorCode::CacheEntryCorrupt, ErrorCode::AssetIntegrityError),
        (ErrorCode::LimitInvalid, ErrorCode::AssetLimitExceeded),
        (
            ErrorCode::ArtifactUnavailable,
            ErrorCode::AssetStorageUnavailable,
        ),
    ] {
        assert_eq!(
            map_asset_store_error(&PlatformError::new(input, "unsafe upstream detail")).code(),
            expected
        );
    }
}

#[test]
fn staged_prepared_bundle_preserves_manifest_digest_size_and_admission() {
    let canonical = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('ok') } }".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bundle.ocb");
    std::fs::write(&path, canonical.bytes()).unwrap();
    let staged = StagedBundle::open(path, BundleLimits::default()).unwrap();
    let prepared = PreparedBundle::prepare(
        &VersionBundle::Staged(staged.clone()),
        BundleLimits::default(),
    )
    .unwrap();

    assert_eq!(prepared.manifest(), staged.manifest());
    assert_eq!(prepared.sha256(), staged.sha256());
    assert_eq!(prepared.size().unwrap(), staged.size());
    assert_eq!(prepared.admission_bytes().unwrap(), 64 * 1024);
}

#[test]
fn runtime_features_prepare_every_builtin_and_enforce_the_pinned_compatibility() {
    let features = VersionRuntimeFeatures {
        compatibility_flags: vec!["nodejs_compat".to_owned()],
        cache: VersionCacheInput {
            default: VersionCachePolicyInput {
                enabled: true,
                cross_version_cache: false,
            },
            entrypoints: BTreeMap::from([(
                "named".to_owned(),
                VersionCachePolicyInput {
                    enabled: true,
                    cross_version_cache: true,
                },
            )]),
        },
        ai: Some(VersionAiInput {
            binding: "AI".to_owned(),
        }),
        images: Some(VersionImagesInput {
            binding: "IMAGES".to_owned(),
        }),
        version_metadata: Some(VersionVersionMetadataInput {
            binding: "VERSION".to_owned(),
            tag: Some("release-1".to_owned()),
        }),
        module_bindings: BTreeMap::from([
            (
                "WASM".to_owned(),
                VersionModuleBindingInput {
                    module: "module.wasm".to_owned(),
                    kind: ModuleBindingKind::WasmModule,
                },
            ),
            (
                "TEXT".to_owned(),
                VersionModuleBindingInput {
                    module: "message.txt".to_owned(),
                    kind: ModuleBindingKind::TextBlob,
                },
            ),
            (
                "DATA".to_owned(),
                VersionModuleBindingInput {
                    module: "data.bin".to_owned(),
                    kind: ModuleBindingKind::DataBlob,
                },
            ),
        ]),
        ..VersionRuntimeFeatures::default()
    };
    assert_eq!(
        validate_compatibility(&features).unwrap(),
        ["nodejs_compat"]
    );
    let (cache, cache_rows, descriptors, rows) = prepare_runtime_features(&features).unwrap();
    assert!(cache.enabled);
    assert_eq!(cache_rows.len(), 2);
    assert_eq!(descriptors.len(), 6);
    assert_eq!(rows.len(), 6);
    assert!(
        descriptors
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(rows.iter().all(|row| row.descriptor_sha256 != [0; 32]));
    assert!(rows.iter().any(|row| row.kind == BuiltinBindingKind::Ai));
    assert!(
        rows.iter()
            .any(|row| row.kind == BuiltinBindingKind::WasmModule)
    );
    assert!(
        rows.iter()
            .any(|row| row.kind == BuiltinBindingKind::TextBlob)
    );
    assert!(
        rows.iter()
            .any(|row| row.kind == BuiltinBindingKind::DataBlob)
    );

    let mut unsupported_date = features.clone();
    unsupported_date.compatibility_date = "2025-01-01".to_owned();
    assert_eq!(
        validate_compatibility(&unsupported_date)
            .unwrap_err()
            .code(),
        ErrorCode::CompatibilityUnsupported
    );
    let mut unsupported_flag = features;
    unsupported_flag.compatibility_flags = vec!["unsupported".to_owned()];
    assert_eq!(
        validate_compatibility(&unsupported_flag)
            .unwrap_err()
            .code(),
        ErrorCode::CompatibilityUnsupported
    );
}

#[test]
fn migration_replay_identity_includes_version_content_and_exact_plan() {
    let account_id = AccountId::generate();
    let worker_id = WorkerId::generate();
    let first = worker_request(account_id, worker_id, b"export default { fetch() {} }");
    let changed = worker_request(
        account_id,
        worker_id,
        b"export default { fetch() { return new Response('changed') } }",
    );
    let first_content = PreparedContent::prepare(&first.content, BundleLimits::default()).unwrap();
    let changed_content =
        PreparedContent::prepare(&changed.content, BundleLimits::default()).unwrap();
    let plan = DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "v1".to_owned(),
        new_sqlite_classes: vec!["Counter".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    let version_id = VersionId::generate();
    let first_fingerprint = request_fingerprint(
        &first,
        &first_content,
        &BTreeMap::new(),
        Some(version_id),
        Some(&plan),
    )
    .unwrap();
    assert_ne!(
        first_fingerprint,
        request_fingerprint(
            &changed,
            &changed_content,
            &BTreeMap::new(),
            Some(version_id),
            Some(&plan),
        )
        .unwrap()
    );
    assert_ne!(
        first_fingerprint,
        request_fingerprint(
            &first,
            &first_content,
            &BTreeMap::new(),
            Some(VersionId::generate()),
            Some(&plan),
        )
        .unwrap()
    );
    let changed_plan = DurableObjectMigrationPlan {
        new_sqlite_classes: vec!["Different".to_owned()],
        ..plan
    };
    assert_ne!(
        first_fingerprint,
        request_fingerprint(
            &first,
            &first_content,
            &BTreeMap::new(),
            Some(version_id),
            Some(&changed_plan),
        )
        .unwrap()
    );
}

#[tokio::test]
async fn binding_preparation_rejects_stale_or_cross_authority_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(
            account,
            "binding-owner",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap()
        .0;
    let mock = MockS3::spawn("open-compute").await;
    let s3 = PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
"#,
        mock.endpoint
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let credentials = resolve_s3_credentials_with(
        &s3,
        &MapEnv::new()
            .with("S3_ACCESS_KEY_ID", "test-access")
            .with("S3_SECRET_ACCESS_KEY", "test-secret"),
    )
    .unwrap();
    let controller = VersionController::new(
        &storage,
        ArtifactStore::new(ObjectBackend::connect_s3(&s3, &credentials, 1024 * 1024).unwrap()),
        Arc::new(|_: ValidationCandidate| async { Ok(()) }),
        BundleLimits::default(),
    );
    let mut request = worker_request(account, worker.id, b"export default {};");
    let version = VersionId::generate();

    let assert_binding_error = |request: &CreateVersionRequest, expected| {
        let Err(error) = controller.prepare_bindings(request, version) else {
            panic!("invalid binding was accepted");
        };
        assert_eq!(error.code(), expected);
    };
    request.bindings.insert(
        "WORKFLOW".to_owned(),
        VersionBindingInput {
            kind: BindingKind::Workflow,
            id: ResourceId::generate(),
            permissions: CanonicalPermissions {
                read: false,
                write: false,
            },
            config: CanonicalBindingConfig::default(),
        },
    );
    assert_binding_error(&request, ErrorCode::WorkflowBindingStale);
    request.bindings.get_mut("WORKFLOW").unwrap().permissions = CanonicalPermissions::default();
    assert_binding_error(&request, ErrorCode::WorkflowBindingStale);

    request.bindings.clear();
    request.bindings.insert(
        "QUEUE".to_owned(),
        VersionBindingInput {
            kind: BindingKind::QueueProducer,
            id: ResourceId::generate(),
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig {
                workflow_class_name: Some("Invalid".to_owned()),
                ..CanonicalBindingConfig::default()
            },
        },
    );
    assert_binding_error(&request, ErrorCode::BindingTypeMismatch);

    request.bindings.clear();
    request.bindings.insert(
        "KV".to_owned(),
        VersionBindingInput {
            kind: BindingKind::KvNamespace,
            id: ResourceId::generate(),
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig {
                workflow_class_name: Some("Invalid".to_owned()),
                ..CanonicalBindingConfig::default()
            },
        },
    );
    assert_binding_error(&request, ErrorCode::BindingTypeMismatch);

    let resources = ResourceRepository::new(storage.db());
    let fingerprint = [7; 32];
    let resource_id = ResourceId::generate();
    let reservation = open_compute_storage::ReserveResourceCreate {
        account_id: account,
        kind: BindingKind::KvNamespace,
        name: "binding-kv",
        idempotency_key: "binding-kv",
        fingerprint_key_id: "test-key",
        request_fingerprint: &fingerprint,
        resource_id,
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 3,
        expires_at_ms: 10_000,
    };
    resources.reserve_create(&reservation, 1_000_000).unwrap();
    request.bindings.clear();
    request.bindings.insert(
        "KV".to_owned(),
        VersionBindingInput {
            kind: BindingKind::KvNamespace,
            id: resource_id,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    assert_binding_error(&request, ErrorCode::ResourceNotReady);
    resources.mark_ready(resource_id, 4).unwrap();
    request.bindings.get_mut("KV").unwrap().kind = BindingKind::R2Bucket;
    assert_binding_error(&request, ErrorCode::ResourceNotFound);

    let other_worker = workers
        .create_worker(
            account,
            "other-binding-owner",
            RequestId::generate(),
            5,
            1_000_000,
        )
        .unwrap()
        .0;
    let namespace_id = ResourceId::generate();
    let namespace = match resources
        .reserve_create(
            &open_compute_storage::ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "cross-authority-do",
                idempotency_key: "cross-authority-do",
                fingerprint_key_id: "test-key",
                request_fingerprint: &[8; 32],
                resource_id: namespace_id,
                driver_schema_version: open_compute_storage::DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 6,
                expires_at_ms: 10_000,
            },
            1_000_000,
        )
        .unwrap()
    {
        open_compute_storage::ResourceCreateReservation::Reserved(value) => value,
        other => panic!("unexpected reservation: {other:?}"),
    };
    DurableObjectRepository::new(&storage)
        .ensure_namespace(&namespace, other_worker.id, "CrossAuthorityCounter")
        .unwrap();
    resources.mark_ready(namespace_id, 7).unwrap();
    request.bindings.clear();
    request.bindings.insert(
        "COUNTER".to_owned(),
        VersionBindingInput {
            kind: BindingKind::DoNamespace,
            id: namespace_id,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    assert_binding_error(&request, ErrorCode::DoNamespaceNotFound);

    let queue_id = QueueId::generate();
    QueueRepository::new(storage.db())
        .insert_creating(
            account,
            queue_id,
            "binding-queue",
            open_compute_storage::QueueConfig::default(),
            5,
        )
        .unwrap();
    request.bindings.clear();
    request.bindings.insert(
        "QUEUE".to_owned(),
        VersionBindingInput {
            kind: BindingKind::QueueProducer,
            id: ResourceId::from_uuid(queue_id.as_uuid()).unwrap(),
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    assert_binding_error(&request, ErrorCode::QueueNotReady);

    request.bindings.clear();
    request.services.insert(
        "MISSING".to_owned(),
        VersionServiceInput {
            target_worker_id: WorkerId::generate(),
            entrypoint: None,
            props: None,
        },
    );
    assert_binding_error(&request, ErrorCode::ServiceBindingDenied);
}
