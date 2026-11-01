use super::*;
use crate::runtime_feature_fixture::RuntimeFeatureFixture;

use open_compute_workers::python_artifact::{
    PreparedPythonIdentity, PythonRuntimePin, publish_prepared_python,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, VersionContent, VersionController, VersionRuntimeFeatures,
};
use std::collections::BTreeMap;

#[tokio::test]
async fn containers_endpoints_preserve_authority_and_require_positive_cleanup() {
    use crate::containers::test_support::{do_authority, fixture};
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = do_authority(&fixture.service, &fixture._artifacts, image, true);
    let repo =
        open_compute_storage::containers::ContainerAuthority::new(fixture._artifacts.storage.db());
    let target = repo
        .target(authority.object.instance_id, authority.app.id, 1)
        .unwrap();
    let declarations = repo
        .version_declarations(authority.object.instance_id, target.spec.worker_version_id)
        .unwrap();

    let state = SourceState {
        source: RuntimeSource::new(
            fixture._artifacts.storage.clone(),
            fixture._artifacts.artifacts.clone(),
            BundleLimits::default(),
            PythonRuntimePin {
                workerd_revision: "a".repeat(40),
                workerd_binary_sha256: "b".repeat(64),
                process_flags: vec!["--experimental".to_owned()],
                pyodide_bundle_sha256: "c".repeat(64),
                runtime_assets_sha256: "d".repeat(64),
            },
        )
        .unwrap(),
        containers: Some(fixture.service.clone()),
    };
    let request = |value: &serde_json::Value| {
        Request::builder()
            .method(Method::POST)
            .header(
                "x-open-compute-startup-generation",
                "container-unit-generation",
            )
            .body(Body::from(serde_json::to_vec(value).unwrap()))
            .unwrap()
    };
    let mut body = serde_json::json!({
        "instanceId": authority.object.instance_id,
        "namespaceResourceId": authority.object.namespace_resource_id,
        "applicationName": authority.app.name,
        "objectId": authority.object.object_id,
        "objectGeneration": authority.object.object_generation,
        "routeGeneration": authority.object.route_generation,
        "descriptorSha256": hex::encode(authority.object.descriptor_sha256),
        "start": true, "forStop": false, "forMonitor": false,
        "startup": {"image":declarations[0].images["main"]},
    });
    let response = container_resolve(State(state.clone()), request(&body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let resolved: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(resolved["lease"]["state"], "starting");
    fixture.running().await;
    body.as_object_mut().unwrap().remove("startup");
    body["start"] = serde_json::json!(false);
    let response = container_resolve(State(state.clone()), request(&body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let running: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(running["lease"]["state"], "running");
    assert_eq!(
        running["lease"]["opaqueEngineKey"],
        resolved["lease"]["opaqueEngineKey"]
    );
    for field in ["start", "forStop", "forMonitor"] {
        body.as_object_mut().unwrap().remove(field);
    }
    body["leaseGeneration"] = running["lease"]["leaseGeneration"].clone();
    body["opaqueEngineKey"] = running["lease"]["opaqueEngineKey"].clone();
    *fixture.protocol.snapshot_rootfs.lock().await =
        crate::containers::test_support::snapshots::rootfs(b"saved");
    fixture
        .protocol
        .image_size
        .store(173, std::sync::atomic::Ordering::SeqCst);
    let snapshot_body = serde_json::json!({"source":body,"name":"checkpoint"});
    let response = container_snapshot(State(state.clone()), request(&snapshot_body)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(snapshot.as_object().unwrap().len(), 1);
    assert_eq!(snapshot["snapshot"].as_object().unwrap().len(), 3);
    assert_eq!(snapshot["snapshot"]["name"], "checkpoint");
    assert!(snapshot["snapshot"]["size"].as_u64().unwrap() > 0);
    let mut invalid = snapshot_body.clone();
    invalid["archive"] = serde_json::json!("untrusted");
    assert_eq!(
        container_snapshot(State(state.clone()), request(&invalid))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        container_begin_stop(State(state.clone()), request(&body))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        container_stop(State(state.clone()), request(&body))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.protocol.terminated.lock().await.len(), 1);
}

#[tokio::test]
async fn container_endpoints_require_an_available_provider() {
    let fixture = Fixture::create().await;
    let frozen = serde_json::json!({
        "instanceId": fixture.base.account,
        "namespaceResourceId": open_compute_core::ResourceId::generate(),
        "applicationName": "application",
        "objectId": "0".repeat(64),
        "objectGeneration": 1,
        "routeGeneration": 1,
        "descriptorSha256": "0".repeat(64),
    });
    let mut resolve_body = frozen.clone();
    resolve_body["start"] = serde_json::json!(false);
    resolve_body["forStop"] = serde_json::json!(false);
    resolve_body["forMonitor"] = serde_json::json!(false);
    let mut stop_body = frozen;
    stop_body["leaseGeneration"] = serde_json::json!(1);
    stop_body["opaqueEngineKey"] = serde_json::json!("a".repeat(64));
    let resolve = container_resolve(
        State(fixture.state.clone()),
        fixture.request(Body::from(serde_json::to_vec(&resolve_body).unwrap())),
    )
    .await;
    assert_eq!(resolve.status(), StatusCode::NOT_FOUND);
    let mut startup_body = resolve_body.clone();
    startup_body["start"] = serde_json::json!(true);
    startup_body["startup"] = serde_json::json!({"environmentVariables": [format!("VALUE={}", "x".repeat(MAX_SOURCE_REQUEST))]});
    assert!(serde_json::to_vec(&startup_body).unwrap().len() > MAX_SOURCE_REQUEST);
    assert_eq!(
        container_resolve(
            State(fixture.state.clone()),
            fixture.request(Body::from(serde_json::to_vec(&startup_body).unwrap()))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        container_resolve(
            State(fixture.state.clone()),
            fixture.request(Body::from(vec![
                b' ';
                MAX_SOURCE_REQUEST + open_compute_storage::crypto::MAX_CONTAINER_STARTUP_BYTES
                    + 1
            ]))
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let stop = container_stop(
        State(fixture.state.clone()),
        fixture.request(Body::from(serde_json::to_vec(&stop_body).unwrap())),
    )
    .await;
    assert_eq!(stop.status(), StatusCode::NOT_FOUND);
    let begin = container_begin_stop(
        State(fixture.state.clone()),
        fixture.request(Body::from(serde_json::to_vec(&stop_body).unwrap())),
    )
    .await;
    assert_eq!(begin.status(), StatusCode::NOT_FOUND);
    let snapshot_body = serde_json::json!({"source":stop_body});
    assert_eq!(
        container_snapshot(
            State(fixture.state.clone()),
            fixture.request(Body::from(serde_json::to_vec(&snapshot_body).unwrap()))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        container_snapshot(
            State(fixture.state.clone()),
            fixture.request(Body::from(vec![b' '; MAX_SOURCE_REQUEST + 1]))
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        container_resolve(
            State(fixture.state.clone()),
            fixture.request(Body::from("{}"))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn prepared_pin_uses_verified_host_binary_and_current_embedded_system_assets() {
    let directory = tempfile::tempdir().unwrap();
    let package = open_compute_runtime::materialize_embedded_runtime(directory.path()).unwrap();
    let runtime = package
        .verify(
            Duration::from_secs(20),
            &open_compute_core::Redactor::new(),
            &directory.path().join("child.lease"),
        )
        .await
        .unwrap();
    let pin = python_runtime_pin(&runtime);
    pin.validate().unwrap();
    assert_eq!(pin.workerd_revision, runtime.lock().revision);
    assert_eq!(pin.workerd_binary_sha256, runtime.binary_sha256());
    assert_eq!(pin.process_flags, runtime.lock().process_flags);
    assert_eq!(
        pin.pyodide_bundle_sha256,
        runtime.lock().pyodide_bundle.bundle_sha256
    );
    assert_eq!(
        pin.runtime_assets_sha256,
        open_compute_runtime::embedded_runtime_assets_sha256()
    );
    assert!(!directory.path().join("child.lease").exists());
}

struct Fixture {
    base: RuntimeFeatureFixture,
    state: SourceState,
    body: serde_json::Value,
}

impl Fixture {
    async fn create() -> Self {
        let features = VersionRuntimeFeatures {
            compatibility_date: "2026-09-08".to_owned(),
            ..Default::default()
        };
        let base = RuntimeFeatureFixture::create(features.clone()).await;
        let pin = PythonRuntimePin {
            workerd_revision: "a".repeat(40),
            workerd_binary_sha256: "b".repeat(64),
            process_flags: vec!["--experimental".to_owned()],
            pyodide_bundle_sha256: "c".repeat(64),
            runtime_assets_sha256: "d".repeat(64),
        };
        let source = RuntimeSource::new(
            base.storage.clone(),
            base.artifacts.clone(),
            BundleLimits::default(),
            pin.clone(),
        )
        .unwrap();
        let validator = {
            let storage = base.storage.clone();
            let artifacts = base.artifacts.clone();
            let source = source.clone();
            move |candidate: ValidationCandidate| {
                let storage = storage.clone();
                let artifacts = artifacts.clone();
                let pin = pin.clone();
                let source = source.clone();
                async move {
                    let snapshot = source
                        .resolve(
                            &loader_key(
                                candidate.instance_id,
                                candidate.worker_id,
                                candidate.version_id,
                            ),
                            &hex::encode(candidate.worker_code_sha256),
                            RuntimeScope::Preparation,
                        )
                        .await?;
                    let identity = PreparedPythonIdentity::from_snapshot(pin, &snapshot)?;
                    publish_prepared_python(
                        &storage,
                        &artifacts,
                        &identity,
                        &SecretBytes::new(b"private native snapshot secret".to_vec()),
                        2,
                        |publish| publish(),
                    )
                    .await?;
                    Ok(())
                }
            }
        };
        let bundle = CanonicalBundle::build(
            "main.py",
            vec![ModuleInput {
                name: "main.py".to_owned(),
                module_type: ModuleType::Python,
                bytes: b"pass\n".to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        let request = CreateVersionRequest {
            instance_id: base.account,
            worker_id: base.worker,
            idempotency_key: "python-source-http".to_owned(),
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
            deployment_source: None,
            observability: None,
            request_id: RequestId::generate(),
            now_ms: 2,
        };
        let result = VersionController::new(
            &base.storage,
            base.artifacts.clone(),
            Arc::new(validator),
            BundleLimits::default(),
        )
        .create_version(request)
        .await
        .unwrap();
        let CreateVersionOutcome::Applied(result) = result else {
            panic!("unexpected replay")
        };
        let key = loader_key(base.account, base.worker, result.version.id);
        let digest = hex::encode(result.version.worker_code_sha256);
        let prepared = source
            .resolve(&key, &digest, RuntimeScope::Runtime)
            .await
            .unwrap()
            .python_prepared_sha256
            .unwrap();

        Self {
            base,
            state: SourceState {
                source,

                containers: None,
            },
            body: serde_json::json!({"key":key,"expectedWorkerCodeSha256":digest,"expectedPreparedSha256":prepared,"scope":"runtime"}),
        }
    }

    fn request(&self, body: Body) -> Request {
        Request::builder()
            .method(Method::POST)
            .uri(PYTHON_SNAPSHOT_PATH)
            .header(
                "x-open-compute-startup-generation",
                "container-unit-generation",
            )
            .body(body)
            .unwrap()
    }
}

#[tokio::test]
async fn python_snapshot_endpoint_returns_only_verified_private_binary() {
    let fixture = Fixture::create().await;
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();

    let listener = bind_runtime_source(&listener_dir.path().join("backend.sock"))
        .await
        .unwrap();
    let address = listener.path().to_owned();
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let source = fixture.state.source.clone();
    let server = tokio::spawn(async move {
        serve_runtime_source(listener, source, None, async move {
            let _ = receiver.await;
        })
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .unix_socket(address.clone())
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let url = format!("http://runtime{PYTHON_SNAPSHOT_PATH}");
    let response = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&fixture.body).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.bytes().await.unwrap(),
        b"private native snapshot secret".as_slice()
    );
    let metadata = {
        let mut metadata = fixture.body.clone();
        metadata
            .as_object_mut()
            .unwrap()
            .remove("expectedPreparedSha256");
        metadata
    };
    let response = client
        .post(format!("http://runtime{SOURCE_PATH}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&metadata).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let wire: serde_json::Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        wire["pythonPreparedSha256"],
        fixture.body["expectedPreparedSha256"]
    );
    assert!(wire.get("pythonSnapshot").is_none());
    fixture
        .base
        ._mock
        .set_fault(open_compute_artifacts::Fault::CorruptBody);
    let response = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&fixture.body).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers()[ERROR_HEADER], "ARTIFACT_INTEGRITY_ERROR");
    assert!(response.bytes().await.unwrap().is_empty());
    drop(client);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn source_request_rejects_invalid_frames() {
    let fixture = Fixture::create().await;
    for body in [
        b"invalid JSON".to_vec(),
        serde_json::to_vec(&serde_json::json!({"key":"x","scope":"runtime"})).unwrap(),
        serde_json::to_vec(&{
            let mut body = fixture.body.clone();
            body["private"] = serde_json::json!(true);
            body
        })
        .unwrap(),
    ] {
        let response = resolve_python(
            State(fixture.state.clone()),
            fixture.request(Body::from(body)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[ERROR_HEADER], "BUNDLE_INVALID");
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }
    for declared in [true, false] {
        let mut request = fixture.request(Body::from(vec![0; MAX_SOURCE_REQUEST + 1]));
        if declared {
            request.headers_mut().insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&(MAX_SOURCE_REQUEST + 1).to_string()).unwrap(),
            );
        }
        let response = resolve_python(State(fixture.state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()[ERROR_HEADER], "BUNDLE_TOO_LARGE");
    }
}
