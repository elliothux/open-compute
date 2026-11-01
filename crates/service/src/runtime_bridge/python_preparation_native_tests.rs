//! Native preparation component regression; ordinary daemon dispatch belongs to its Gate.

use super::*;
use crate::local_extensions::LocalExtensionRegistry;
use crate::runtime_feature_fixture::{artifact_store, storage_config};
use crate::service_invocations::ServiceInvocationRegistry;
use open_compute_artifacts::MockS3;
use open_compute_core::{RequestId, SystemClock};
use open_compute_runtime::HostExtensionBrokerRegistry;
use open_compute_workers::python_artifact::restore_prepared_python;
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, RuntimeValidator, VersionContent, VersionController, VersionRuntimeFeatures,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Evidence(Option<tempfile::TempDir>);

impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.0.take()
        {
            eprintln!(
                "native preparation evidence: {}",
                directory.keep().display()
            );
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureManifest {
    content_type: String,
    sha256: String,
    modules: Vec<CaptureModule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureModule {
    name: String,
    upload_name: String,
    mime: String,
    size: usize,
    sha256: String,
}

async fn captured_main(root: &Path) -> Vec<ModuleInput> {
    let capture = root.join("test/fixtures/python-main");
    let manifest: CaptureManifest =
        serde_json::from_slice(&std::fs::read(capture.join("manifest.json")).unwrap()).unwrap();
    let bytes = std::fs::read(capture.join("upload.multipart")).unwrap();
    assert_eq!(hex::encode(Sha256::digest(&bytes)), manifest.sha256);
    let boundary = multer::parse_boundary(&manifest.content_type).unwrap();
    let mut parts = multer::Multipart::new(
        futures::stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from(bytes)) }),
        boundary,
    );
    let mut modules = Vec::new();
    while let Some(part) = parts.next_field().await.unwrap() {
        let name = part.name().unwrap();
        if name == "metadata" {
            continue;
        }
        let expected = manifest
            .modules
            .iter()
            .find(|m| m.upload_name == name)
            .unwrap();
        assert!(
            !modules
                .iter()
                .any(|m: &ModuleInput| m.name == expected.name)
        );
        assert_eq!(part.content_type().unwrap().as_ref(), expected.mime);
        let bytes = part.bytes().await.unwrap().to_vec();
        assert_eq!(bytes.len(), expected.size);
        assert_eq!(hex::encode(Sha256::digest(&bytes)), expected.sha256);
        modules.push(ModuleInput {
            name: expected.name.clone(),
            module_type: match expected.mime.as_str() {
                "text/x-python" => ModuleType::Python,
                "application/javascript+module" => ModuleType::EsModule,
                "application/octet-stream" => ModuleType::Data,
                _ => panic!("unexpected captured module MIME"),
            },
            bytes,
        });
    }
    assert_eq!(modules.len(), manifest.modules.len());
    modules
}

#[tokio::test]
async fn fresh_native_preparation_publishes_once_and_reaps_after_import_rejection() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let evidence_root = root.join(".temp/python-preparation-native-run");
    std::fs::create_dir_all(&evidence_root).unwrap();
    let evidence = Evidence(Some(
        tempfile::Builder::new()
            .prefix("fresh-")
            .tempdir_in(std::fs::canonicalize(evidence_root).unwrap())
            .unwrap(),
    ));
    let directory = evidence.0.as_ref().unwrap().path();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&directory.join("data")), &SystemClock).unwrap(),
    );
    let account = storage.identity().instance_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "native-preparation", RequestId::generate(), 1, 100)
        .unwrap()
        .0
        .id;
    let mock = Arc::new(MockS3::spawn("open-compute").await);
    let artifacts = artifact_store(&mock);
    let runtime_dir = storage.data_dir().runtime_dir();
    let package = open_compute_runtime::materialize_embedded_runtime(&runtime_dir).unwrap();
    let runtime = package
        .verify(
            Duration::from_secs(20),
            &open_compute_core::Redactor::new(),
            &runtime_dir.join("native-test-verify.lease"),
        )
        .await
        .unwrap();
    // materialize/verify enforces the formal embedded binary, assets and Pyodide pin.
    let pin = python_runtime_pin(&runtime);
    let source = RuntimeSource::new(
        storage.clone(),
        artifacts.clone(),
        BundleLimits::default(),
        pin.clone(),
    )
    .unwrap();
    let source_listener_dir = tempfile::tempdir_in("/tmp").unwrap();

    let source_listener = bind_runtime_source(&source_listener_dir.path().join("backend.sock"))
        .await
        .unwrap();
    let source_port = source_listener.path().to_owned();
    let services = ["runtime-source", "binding-backend", "observability-backend"]
        .map(|name| ExternalServiceAddress::unix(name, &source_port).unwrap());
    let (stop, stopped) = oneshot::channel();
    let source_server = tokio::spawn(serve_runtime_source(
        source_listener,
        source.clone(),
        None,
        async {
            let _ = stopped.await;
        },
    ));
    let broker = Arc::new(
        HostExtensionBroker::new(
            HostExtensionBrokerRegistry::new(),
            Arc::new(LocalExtensionRegistry::empty()),
            Arc::new(ServiceInvocationRegistry::new(
                storage.clone(),
                VersionPins::new(),
            )),
            &storage,
            open_compute_core::Redactor::new(),
        )
        .unwrap(),
    );
    // This component supplies parent-generation admission; it does not run the daemon.
    let mut transport =
        WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock"));
    transport.test_generation = Some(open_compute_core::StartupId::generate());
    let preparation = Arc::new(PythonPreparation {
        storage: storage.clone(),
        artifacts: artifacts.clone(),
        source: source.clone(),
        package,
        runtime_config: RuntimeConfig::default(),
        durable_objects: DurableObjectsConfig::default(),
        services,
        broker,
        serial: tokio::sync::Mutex::new(()),
    });
    let validator: Arc<dyn RuntimeValidator> = {
        let preparation = preparation.clone();
        let transport = transport.clone();
        let mock = mock.clone();
        Arc::new(move |candidate: ValidationCandidate| {
            let preparation = preparation.clone();
            let transport = transport.clone();
            let mock = mock.clone();
            async move {
                preparation
                    .prepare(candidate.clone(), transport.clone())
                    .await?;
                let original = WorkerRepository::new(preparation.storage.db())
                    .version_snapshot(
                        candidate.instance_id,
                        candidate.worker_id,
                        candidate.version_id,
                        true,
                    )?
                    .python_prepared
                    .unwrap();
                let object_count = mock.object_count();
                // Retained recovery verifies actual native snapshot AEAD and identity;
                // the second call must not start another process or mutate authority.
                preparation.prepare(candidate.clone(), transport).await?;
                let retained = WorkerRepository::new(preparation.storage.db())
                    .version_snapshot(
                        candidate.instance_id,
                        candidate.worker_id,
                        candidate.version_id,
                        true,
                    )?
                    .python_prepared
                    .unwrap();
                assert_eq!(retained, original);
                assert_eq!(mock.object_count(), object_count);
                Ok(())
            }
        })
    };
    let original = captured_main(&root).await;
    let mut rejected = original.clone();
    rejected
        .iter_mut()
        .find(|m| m.name == "main.py")
        .unwrap()
        .bytes = b"raise RuntimeError('private native import traceback')\n".to_vec();
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let mut ready = None;
    for (key, modules) in [("native-success", original), ("native-rejected", rejected)] {
        let bundle = CanonicalBundle::build("main.py", modules, BundleLimits::default()).unwrap();
        let outcome = controller
            .create_version(CreateVersionRequest {
                instance_id: account,
                worker_id: worker,
                idempotency_key: key.to_owned(),
                content: VersionContent::Worker {
                    bundle: bundle.into_bytes().into(),
                    assets: None,
                },
                // No handler is invoked by preparation; product bindings are exercised by Main Gate.
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                bindings: BTreeMap::new(),
                services: BTreeMap::new(),
                runtime_features: VersionRuntimeFeatures {
                    compatibility_date: "2026-09-08".to_owned(),
                    compatibility_flags: vec![
                        "python_workers".to_owned(),
                        "enable_python_external_sdk".to_owned(),
                        "python_dedicated_snapshot".to_owned(),
                    ],
                    ..Default::default()
                },
                queue_consumers: Vec::new(),
                crons: Vec::new(),
                deployment_source: None,
                observability: None,
                request_id: RequestId::generate(),
                now_ms: 20,
            })
            .await;
        // Both outcomes must reap before returning; broker completion is awaited by production.
        for lease in ["python-compile.lease", "python-prepare.lease"] {
            assert!(
                !runtime_dir.join(lease).exists(),
                "retained lease after {key}"
            );
        }
        if key == "native-success" {
            let CreateVersionOutcome::Applied(result) = outcome.unwrap() else {
                panic!("replay");
            };
            assert_eq!(result.version.state, VersionState::Ready);
            assert!(result.deployment.is_none());
            let authority = WorkerRepository::new(storage.db())
                .version_snapshot(account, worker, result.version.id, false)
                .unwrap();
            let record = authority.python_prepared.unwrap();
            let identity: PreparedPythonIdentity =
                serde_json::from_slice(&record.identity_json).unwrap();
            assert_eq!(identity.runtime.pin, pin);
            let bytes = restore_prepared_python(&record, &identity, &artifacts, storage.crypto())
                .await
                .unwrap();
            assert!(bytes.expose().len() >= 16);
            assert!(mock.object_count() >= 2);
            ready = Some(result.version.id);
        } else {
            let error = outcome.unwrap_err();
            assert_eq!(error.code(), ErrorCode::BundleRuntimeInvalid);
            assert!(!error.to_string().contains("traceback"));
            let versions = WorkerRepository::new(storage.db())
                .list_versions(account, worker)
                .unwrap();
            let failed = versions.iter().find(|v| v.id != ready.unwrap()).unwrap();
            assert_eq!(failed.state, VersionState::Rejected);
            assert_eq!(
                WorkerRepository::new(storage.db())
                    .version_snapshot(account, worker, failed.id, true)
                    .unwrap_err()
                    .code(),
                ErrorCode::VersionNotReady
            );
            let connection = rusqlite::Connection::open_with_flags(
                storage.data_dir().control_db_path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let prepared_count = connection
                .query_row(
                    "SELECT COUNT(*) FROM version_python_prepared WHERE version_id=?1",
                    [failed.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            assert_eq!(prepared_count, 0);
        }
    }
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), source_server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
