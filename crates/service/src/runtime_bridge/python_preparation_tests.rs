use super::*;
use bytes::Bytes;
use futures::stream;
use hyper::body::Frame;

fn snapshot_response(body: Body, size: &str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_LENGTH, size)
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn private_snapshot_collection_bounds_and_sanitizes_all_response_frames() {
    let snapshot = collect_snapshot(snapshot_response(
        Body::from_stream(stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"first-eight")),
            Ok(Bytes::from_static(b"second-eight")),
        ])),
        "23",
    ))
    .await
    .unwrap();
    assert_eq!(snapshot.expose(), b"first-eightsecond-eight");
    assert!(!format!("{snapshot:?}").contains("first-eight"));
    for length in [
        "",
        "-1",
        "+16",
        "0016",
        "15",
        "134217729",
        "184467440737095516160",
    ] {
        assert_eq!(
            collect_snapshot(snapshot_response(Body::empty(), length))
                .await
                .unwrap_err()
                .code(),
            ErrorCode::RuntimeUnavailable
        );
    }
    for field in [
        header::CONTENT_LENGTH,
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
    ] {
        let mut response = snapshot_response(Body::empty(), "16");
        response.headers_mut().remove(field);
        assert!(collect_snapshot(response).await.is_err());
    }
    for (field, value) in [
        (header::CONTENT_TYPE, "text/plain"),
        (header::CACHE_CONTROL, "public"),
        (header::CONTENT_ENCODING, "identity"),
    ] {
        let mut response = snapshot_response(Body::empty(), "16");
        response
            .headers_mut()
            .insert(field, HeaderValue::from_static(value));
        assert!(collect_snapshot(response).await.is_err());
    }
    for bytes in [
        b"too short".as_slice(),
        b"much more than sixteen bytes".as_slice(),
    ] {
        assert!(
            collect_snapshot(snapshot_response(Body::from(bytes.to_vec()), "16"))
                .await
                .is_err()
        );
    }
    let failed = Body::from_stream(stream::iter([
        Ok(Bytes::from_static(b"partial")),
        Err(std::io::Error::other("private traceback secret path")),
    ]));
    let error = collect_snapshot(snapshot_response(failed, "16"))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("traceback"));
    let trailers = Body::new(http_body_util::StreamBody::new(stream::iter([Ok::<
        _,
        std::io::Error,
    >(
        Frame::trailers(HeaderMap::new()),
    )])));
    assert!(
        collect_snapshot(snapshot_response(trailers, "16"))
            .await
            .is_err()
    );
    for (status, code, expected) in [
        (
            422,
            "BUNDLE_RUNTIME_INVALID",
            ErrorCode::BundleRuntimeInvalid,
        ),
        (409, "VERSION_NOT_READY", ErrorCode::VersionNotReady),
        (503, "ARTIFACT_UNAVAILABLE", ErrorCode::ArtifactUnavailable),
        (503, "RUNTIME_UNAVAILABLE", ErrorCode::RuntimeUnavailable),
        (404, "private unknown secret", ErrorCode::RuntimeUnavailable),
    ] {
        let response = Response::builder()
            .status(status)
            .header(ERROR_HEADER, code)
            .body(Body::from("private traceback secret"))
            .unwrap();
        let error = collect_snapshot(response).await.unwrap_err();
        assert_eq!(error.code(), expected);
        assert!(!error.to_string().contains("secret"));
    }
}

#[test]
fn cancellation_and_deadline_checks_reject_expiry_and_dropped_uploads() {
    assert!(remaining(Instant::now() + Duration::from_secs(100)).is_ok());
    assert!(remaining(Instant::now()).is_err());
    let (sender, mut receiver) = oneshot::channel();
    assert!(ensure_not_cancelled(&mut receiver).is_ok());
    drop(sender);
    assert!(ensure_not_cancelled(&mut receiver).is_err());
    let (sender, mut receiver) = oneshot::channel();
    sender.send(()).unwrap();
    assert!(ensure_not_cancelled(&mut receiver).is_err());
}

#[tokio::test]
async fn prepare_http_uses_the_private_identity_and_preserves_failure_status_headers() {
    let candidate = ValidationCandidate {
        instance_id: InstanceId::generate(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        worker_code_sha256: [2; 32],
    };

    let expected = candidate.clone();
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let router = Router::new().route(
        "/internal/prepare-python",
        post(move |request: Request| {
            let expected = expected.clone();
            async move {
                assert_eq!(request.method(), Method::POST);

                assert_eq!(
                    request.headers()["x-open-compute-instance-id"],
                    expected.instance_id.to_string()
                );
                assert_eq!(
                    request.headers()["x-open-compute-loader-key"],
                    loader_key(
                        expected.instance_id,
                        expected.worker_id,
                        expected.version_id
                    )
                );
                assert_eq!(
                    request.headers()["x-open-compute-worker-code-sha256"],
                    "02".repeat(32)
                );
                assert_eq!(request.headers()["x-open-compute-route-generation"], "0");
                assert!(to_bytes(request.into_body(), 1).await.unwrap().is_empty());
                Response::builder()
                    .status(422)
                    .header(ERROR_HEADER, "BUNDLE_RUNTIME_INVALID")
                    .body(Body::from("private import traceback"))
                    .unwrap()
            }
        }),
    );
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let transport = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    let error = PythonPreparation::request_snapshot(
        &transport,
        &port,
        &candidate,
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BundleRuntimeInvalid);
    assert!(!error.to_string().contains("traceback"));
    assert!(
        PythonPreparation::request_snapshot(&transport, &port, &candidate, Instant::now())
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    // A lost preparation listener cannot expose the connector's address/error.
    let error = PythonPreparation::request_snapshot(
        &transport,
        &port,
        &candidate,
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
    assert!(!error.to_string().contains(&port.display().to_string()));
}

#[tokio::test]
async fn preparation_http_collects_complete_snapshots_and_bounds_stalled_or_truncated_responses() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let candidate = ValidationCandidate {
        instance_id: InstanceId::generate(),
        worker_id: WorkerId::generate(),
        version_id: VersionId::generate(),
        worker_code_sha256: [2; 32],
    };

    let transport = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    let bytes = b"0123456789abcdef01234567";
    for mode in ["complete", "stalled_headers", "stalled_body", "truncated"] {
        let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
        let listener_path = listener_dir.path().join("runtime.sock");
        let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
        let port = listener_path.clone();
        let (arrived, request_arrived) = oneshot::channel();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Observe the request before deliberately withholding headers/body;
            // no sleep or assumed connection timing establishes the fault point.
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            assert!(request.starts_with(b"POST /internal/prepare-python HTTP/1.1\r\n"));
            if mode != "stalled_headers" {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nContent-Length: 24\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                socket.write_all(&bytes[..16]).await.unwrap();
                if mode == "complete" {
                    socket.write_all(&bytes[16..]).await.unwrap();
                }
            }
            arrived.send(()).unwrap();
            if mode != "truncated" {
                let _ = stopped.await;
            }
            drop(socket);
        });
        let deadline = Instant::now() + Duration::from_millis(500);
        let pending = {
            let transport = transport.clone();
            let candidate = candidate.clone();
            let port = port.clone();
            tokio::spawn(async move {
                PythonPreparation::request_snapshot(&transport, &port, &candidate, deadline).await
            })
        };
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            request_arrived.await.unwrap();
            pending.await.unwrap()
        })
        .await;
        // Release the fixture even on failure, and reap before asserting results.
        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        let result = result.unwrap();
        if mode == "complete" {
            assert_eq!(result.unwrap().expose(), bytes);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code(), ErrorCode::RuntimeUnavailable, "{mode}");
            assert!(!error.to_string().contains(&port.display().to_string()));
            assert!(!error.to_string().contains("0123456789abcdef"));
            if mode.starts_with("stalled_") {
                assert!(Instant::now() >= deadline, "{mode}");
            }
        }
    }
}

#[tokio::test]
async fn retained_preparation_reuses_verified_authority_and_fences_runtime_generation() {
    use crate::local_extensions::LocalExtensionRegistry;
    use crate::runtime_feature_fixture::RuntimeFeatureFixture;
    use crate::service_invocations::ServiceInvocationRegistry;
    use open_compute_runtime::HostExtensionBrokerRegistry;
    use open_compute_workers::{
        BundleLimits, CanonicalBundle, CreateVersionRequest, ModuleInput, ModuleType,
        VersionContent, VersionController, VersionRuntimeFeatures,
    };
    use std::collections::BTreeMap;

    let features = VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".to_owned(),
        ..Default::default()
    };
    let fixture = RuntimeFeatureFixture::create(features.clone()).await;
    let package = open_compute_runtime::materialize_embedded_runtime(
        &fixture.storage.data_dir().runtime_dir(),
    )
    .unwrap();
    let runtime = package
        .verify(
            Duration::from_secs(20),
            &open_compute_core::Redactor::new(),
            &fixture
                .storage
                .data_dir()
                .runtime_dir()
                .join("verify-test.lease"),
        )
        .await
        .unwrap();
    let source = RuntimeSource::new(
        fixture.storage.clone(),
        fixture.artifacts.clone(),
        BundleLimits::default(),
        python_runtime_pin(&runtime),
    )
    .unwrap();
    let services = ["runtime-source", "binding-backend", "observability-backend"]
        .map(|name| ExternalServiceAddress::unix(name, Path::new("/tmp/missing.sock")).unwrap());
    let broker = Arc::new(
        HostExtensionBroker::new(
            HostExtensionBrokerRegistry::new(),
            Arc::new(LocalExtensionRegistry::empty()),
            Arc::new(ServiceInvocationRegistry::new(
                fixture.storage.clone(),
                VersionPins::new(),
            )),
            &fixture.storage,
            open_compute_core::Redactor::new(),
        )
        .unwrap(),
    );
    let mut transport = WorkerdTransport::for_test_endpoint(Path::new("/tmp/missing.sock"));
    let generation = open_compute_core::StartupId::generate();
    transport.test_generation = Some(generation);
    let clone_before_configure = transport.clone();
    let coordinator = PythonPreparation {
        storage: fixture.storage.clone(),
        artifacts: fixture.artifacts.clone(),
        source: source.clone(),
        package,
        runtime_config: RuntimeConfig::default(),
        durable_objects: DurableObjectsConfig::default(),
        services,
        broker,
        serial: tokio::sync::Mutex::new(()),
    };
    let js_candidate = ValidationCandidate {
        instance_id: fixture.account,
        worker_id: fixture.worker,
        version_id: fixture.version,
        worker_code_sha256: WorkerRepository::new(fixture.storage.db())
            .get_version(fixture.account, fixture.worker, fixture.version)
            .unwrap()
            .worker_code_sha256,
    };
    // These failures precede Source resolution, compilation and any child lease.
    let no_owner = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    let mut no_generation = transport.clone();
    no_generation.test_generation = None;
    for rejected in [no_owner, no_generation] {
        let (_cancel, cancelled) = oneshot::channel();
        assert_eq!(
            coordinator
                .run(js_candidate.clone(), rejected, cancelled)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::RuntimeUnavailable,
        );
    }
    assert_eq!(fixture._mock.object_count(), 1);
    for lease in ["python-compile.lease", "python-prepare.lease"] {
        assert!(
            !fixture
                .storage
                .data_dir()
                .runtime_dir()
                .join(lease)
                .exists()
        );
    }
    transport.configure_python_preparation(coordinator).unwrap();
    let preparation = clone_before_configure
        .python_preparation
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    // Ordinary JS validation never needs a preparation process or generation.
    preparation
        .prepare(
            js_candidate.clone(),
            WorkerdTransport::new(Arc::new(Mutex::new(None))),
        )
        .await
        .unwrap();
    let mut unknown = js_candidate;
    unknown.version_id = VersionId::generate();
    assert!(
        preparation
            .prepare(unknown, transport.clone())
            .await
            .is_err()
    );
    assert_eq!(
        preparation
            .with_generation(&transport, generation, || Ok(37))
            .unwrap(),
        37
    );
    let validator = {
        let preparation = preparation.clone();
        let transport = transport.clone();
        let source = source.clone();
        let pin = python_runtime_pin(&runtime);
        move |candidate: ValidationCandidate| {
            let preparation = preparation.clone();
            let transport = transport.clone();
            let source = source.clone();
            let pin = pin.clone();
            async move {
                let key = loader_key(
                    candidate.instance_id,
                    candidate.worker_id,
                    candidate.version_id,
                );
                let expected = hex::encode(candidate.worker_code_sha256);
                let snapshot = source
                    .resolve(&key, &expected, RuntimeScope::Preparation)
                    .await?;
                let identity = PreparedPythonIdentity::from_snapshot(pin, &snapshot)?;
                let record = publish_prepared_python(
                    &preparation.storage,
                    &preparation.artifacts,
                    &identity,
                    &SecretBytes::new(b"fixture native snapshot bytes".to_vec()),
                    2,
                    |publish| publish(),
                )
                .await?;
                // This exercises retained recovery with real SQLite/SigV4/AEAD. The
                // parent generation observation and native snapshot bytes are fixtures.
                preparation
                    .prepare(candidate.clone(), transport.clone())
                    .await?;
                assert_eq!(
                    WorkerRepository::new(preparation.storage.db())
                        .version_snapshot(
                            candidate.instance_id,
                            candidate.worker_id,
                            candidate.version_id,
                            true
                        )?
                        .python_prepared,
                    Some(record)
                );
                let held = preparation.serial.lock().await;
                let (cancel, cancelled) = oneshot::channel();
                drop(cancel);
                assert!(
                    preparation
                        .run(candidate, transport, cancelled)
                        .await
                        .is_err()
                );
                drop(held);
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
    let result = VersionController::new(
        &fixture.storage,
        fixture.artifacts.clone(),
        Arc::new(validator),
        BundleLimits::default(),
    )
    .create_version(CreateVersionRequest {
        instance_id: fixture.account,
        worker_id: fixture.worker,
        idempotency_key: "retained-preparation".to_owned(),
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
    })
    .await
    .unwrap();
    let open_compute_workers::CreateVersionOutcome::Applied(result) = result else {
        panic!("unexpected replay")
    };
    let candidate = ValidationCandidate {
        instance_id: fixture.account,
        worker_id: fixture.worker,
        version_id: result.version.id,
        worker_code_sha256: result.version.worker_code_sha256,
    };
    assert_eq!(
        preparation
            .prepare(candidate, transport.clone())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    assert_eq!(
        fixture._mock.object_count(),
        3,
        "retained recovery adds no immutable objects"
    );
    transport.test_generation = Some(open_compute_core::StartupId::generate());
    assert!(
        preparation
            .with_generation(&transport, generation, || Ok(()))
            .is_err()
    );
}
