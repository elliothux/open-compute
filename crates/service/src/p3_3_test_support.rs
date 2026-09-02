use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{
    AccountId, CacheConfig, PlatformConfig, RequestId, StartupId, StorageConfig, SystemClock,
    VersionId, WorkerId,
};
use open_compute_storage::{
    BuiltinBindingKind, PlatformStorage, WorkerRepository, version_runtime_features,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, RuntimeValidator, ValidationCandidate, VersionContent, VersionController,
    VersionRuntimeFeatures,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

pub(super) struct RuntimeFeatureFixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) _mock: MockS3,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) artifacts: ArtifactStore,
    pub(super) artifact_cache: Arc<ArtifactCache>,
    pub(super) account: AccountId,
    pub(super) worker: WorkerId,
    pub(super) version: VersionId,
    pub(super) descriptor_sha256: String,
    pub(super) ai_descriptor_sha256: Option<String>,
    pub(super) images_descriptor_sha256: Option<String>,
}

impl RuntimeFeatureFixture {
    pub(super) async fn create(features: VersionRuntimeFeatures) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
                .unwrap(),
        );
        let account = storage.identity().default_account_id;
        let worker = WorkerRepository::new(storage.db())
            .create_worker(
                account,
                "runtime-features",
                RequestId::generate(),
                1,
                1_000_000,
            )
            .unwrap()
            .0
            .id;
        let mock = MockS3::spawn("open-compute").await;
        let artifacts = artifact_store(&mock);
        let artifact_cache = Arc::new(
            ArtifactCache::open(
                storage.data_dir().artifact_cache_dir(),
                CacheConfig::default(),
                StartupId::generate(),
            )
            .unwrap(),
        );
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: b"export default { fetch() { return new Response('ok'); } };".to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        let request = CreateVersionRequest {
            account_id: account,
            worker_id: worker,
            idempotency_key: "runtime-features".to_owned(),
            content: VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            runtime_features: features,
            queue_consumers: Vec::new(),
            crons: Vec::new(),
            deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
            request_id: RequestId::generate(),
            now_ms: 1_000,
        };
        let result = VersionController::new(
            &storage,
            artifacts.clone(),
            Arc::new(AcceptAllValidator),
            BundleLimits::default(),
        )
        .create_version(request)
        .await
        .unwrap();
        let CreateVersionOutcome::Applied(result) = result else {
            panic!("first version unexpectedly replayed");
        };
        let (_, builtins) = version_runtime_features(storage.db(), result.version.id).unwrap();
        let images_descriptor_sha256 = builtins
            .iter()
            .find(|binding| binding.kind == BuiltinBindingKind::Images)
            .map(|binding| hex::encode(binding.descriptor_sha256));
        let ai_descriptor_sha256 = builtins
            .iter()
            .find(|binding| binding.kind == BuiltinBindingKind::Ai)
            .map(|binding| hex::encode(binding.descriptor_sha256));
        Self {
            _temp: temp,
            _mock: mock,
            storage,
            artifacts,
            artifact_cache,
            account,
            worker,
            version: result.version.id,
            descriptor_sha256: hex::encode(result.version.worker_code_sha256),
            ai_descriptor_sha256,
            images_descriptor_sha256,
        }
    }
}

struct AcceptAllValidator;

impl RuntimeValidator for AcceptAllValidator {
    fn validate(
        &self,
        _candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }

    fn validate_entrypoint(
        &self,
        _candidate: ValidationCandidate,
        _entrypoint: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 3000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap())
}
