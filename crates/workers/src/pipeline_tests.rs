use super::*;
use crate::assets::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, HtmlHandling, NotFoundHandling,
};

fn deployment_assets(binding: Option<&str>, worker_first: RunWorkerFirst) -> DeploymentAssets {
    DeploymentAssets {
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

fn assets_only_request(assets: &DeploymentAssets) -> CreateDeploymentRequest {
    CreateDeploymentRequest {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        idempotency_key: "asset-test".to_owned(),
        content: DeploymentContent::AssetsOnly {
            assets: assets.clone(),
        },
        compatibility_date: "2026-08-22".to_owned(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({}),
        promote: false,
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
        deployment_id: DeploymentId::generate(),
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
    assert_eq!(invariant().code(), ErrorCode::DeploymentInvariantViolation);
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
    let assets = deployment_assets(Some("ASSETS"), RunWorkerFirst::All(false));
    let content = PreparedContent::AssetsOnly {
        assets: assets.clone(),
    };
    let request = assets_only_request(&assets);
    assert!(validate_asset_content(&request, &content, &BTreeMap::new()).is_ok());
    assert_eq!(content.kind(), DeploymentContentKind::AssetsOnly);
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

    let mut request = assets_only_request(&deployment_assets(None, RunWorkerFirst::All(false)));
    let content = PreparedContent::AssetsOnly {
        assets: deployment_assets(None, RunWorkerFirst::All(false)),
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

    let request = assets_only_request(&deployment_assets(None, RunWorkerFirst::All(true)));
    let content = PreparedContent::AssetsOnly {
        assets: deployment_assets(None, RunWorkerFirst::All(true)),
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
