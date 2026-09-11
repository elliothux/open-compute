use crate::pipeline::{
    idempotency_ref_id, stable_validation_code, validate_idempotency_key, validate_secret_set,
};
use crate::runtime_source::{invariant as source_invariant, map_artifact_error, not_ready};
use crate::*;
use open_compute_artifacts::{
    ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, MockS3, ObjectBackend,
    resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    AccountId, BindingKind, CacheConfig, DataConfig, ErrorCode, PlatformConfig, RequestId,
    SecretString, StartupId, VersionId, WorkerId,
};
use open_compute_storage::{PlatformStorage, VersionState, WorkerRepository};
use sha2::Digest as _;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[path = "environment_storage_tests.rs"]
mod environment_storage_tests;

struct AcceptAllValidator;

impl RuntimeValidator for AcceptAllValidator {
    fn validate(
        &self,
        _candidate: ValidationCandidate,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>,
    > {
        Box::pin(async { Ok(()) })
    }

    fn validate_entrypoint(
        &self,
        _candidate: ValidationCandidate,
        _entrypoint: String,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>,
    > {
        Box::pin(async { Ok(()) })
    }
}

fn module(name: &str, bytes: &[u8]) -> ModuleInput {
    ModuleInput {
        name: name.to_owned(),
        module_type: ModuleType::EsModule,
        bytes: bytes.to_vec(),
    }
}

fn rewrite_bundle(
    bytes: &[u8],
    mutate: impl FnOnce(&mut WorkerBundleManifest, &mut Vec<u8>),
) -> Vec<u8> {
    let manifest_len = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut manifest: WorkerBundleManifest =
        serde_json::from_slice(&bytes[12..12 + manifest_len]).unwrap();
    let mut blob = bytes[12 + manifest_len..].to_vec();
    mutate(&mut manifest, &mut blob);
    let encoded = serde_json::to_vec(&manifest).unwrap();
    let mut out = b"OCWB\0\x01\0\0".to_vec();
    out.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    out.extend_from_slice(&encoded);
    out.extend_from_slice(&blob);
    out
}

fn staged_error(bytes: &[u8], limits: BundleLimits) -> ErrorCode {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bundle.ocwb");
    fs::write(&path, bytes).unwrap();
    StagedBundle::open(path, limits).unwrap_err().code()
}

mod bundle_is_canonical_and_round_trips;

mod staged_bundle_incrementally_verifies_digest_layout_and_trailing_bytes;

mod staged_bundle_rejects_filesystem_framing_and_module_corruption;

mod bundle_rejects_path_digest_offset_and_trailing_attacks;

mod bundle_limits_and_main_type_are_enforced;

mod bundle_build_and_parse_cover_structural_validation_matrix;

mod bundle_verifiers_reject_every_canonical_manifest_boundary;

mod descriptor_binds_every_runtime_effective_input;

mod vars_reject_reserved_names_and_prototype_keys;

mod descriptor_env_date_and_secret_validation_matrix;

mod loader_key_is_strict;

mod version_pins_timeout_unfence_retire_and_debug_paths;

mod version_pipeline_helper_contracts_cover_failure_code_matrix;

fn storage_config(root: &std::path::Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn s3_config(endpoint: &str) -> open_compute_core::S3Config {
    PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 1500
"#
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone()
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = s3_config(&mock.endpoint);
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    key: &str,
    secret: &str,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module(
                "index.js",
                b"export default { fetch() { return new Response('hello'); } };",
            ),
            ModuleInput {
                name: "index.js.map".to_owned(),
                module_type: ModuleType::SourceMap,
                bytes: br#"{"version":3,"sources":["index.ts"],"names":[],"mappings":""}"#.to_vec(),
            },
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!("production"));
    let mut secrets = BTreeMap::new();
    secrets.insert("API_TOKEN".to_owned(), SecretString::new(secret));
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets,
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: 10_000,
    }
}

#[path = "runtime_snapshot_tests.rs"]
mod runtime_snapshot;

mod fixed_upload_finalize_resumes_one_cancelled_validating_version;

mod assets_only_pipeline_commits_real_refs_without_fabricating_worker_code;

mod version_products_validate_ready_queue_dlq_entrypoint_counts_and_crons;

mod runtime_source_error_mapping_is_stable_and_sanitized;

mod shared_artifact_gc_waits_for_last_version_reference;

mod validation_failure_is_rejected_replayed_and_never_promoted;
