use super::*;
use crate::assets::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, HtmlHandling, NotFoundHandling,
};
use crate::{ModuleInput, ModuleType};

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
