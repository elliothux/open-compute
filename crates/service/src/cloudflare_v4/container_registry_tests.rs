use super::*;
use crate::containers::test_support::{Fixture, authority, fixture};
use serde_json::json;
use tower::ServiceExt as _;

fn app(fixture: &Fixture) -> Router {
    crate::cloudflare_v4::router(
        fixture.state.clone(),
        crate::cloudflare_v4::storage_router(),
    )
    .merge(router())
    .with_state(fixture.state.clone())
}
async fn body(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}
fn request(method: Method, path: &str, authorization: &str, bytes: Vec<u8>) -> Request {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, authorization)
        .header(header::HOST, "registry.example.test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap()
}

async fn upload(
    fixture: &Fixture,
    application: &Router,
    repository: &str,
    auth: &str,
    bytes: &[u8],
) -> String {
    let start = application
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/{repository}/blobs/uploads/"),
            auth,
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::ACCEPTED);
    let location = start.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let middle = bytes.len() / 2;
    let appended = application
        .clone()
        .oneshot(request(
            Method::PATCH,
            &location,
            auth,
            bytes[..middle].to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(appended.status(), StatusCode::ACCEPTED);
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
    let finish = application
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("{location}?digest={digest}"),
            auth,
            bytes[middle..].to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(finish.status(), StatusCode::CREATED);
    assert_eq!(finish.headers()["docker-content-digest"], digest);
    assert!(
        ContainerAuthority::new(fixture.service.storage.db())
            .registry_blobs(fixture.service.instance_id(), repository)
            .unwrap()
            .last()
            .unwrap()
            .size
            > 0
    );
    digest
}

#[tokio::test]
async fn container_registry_preserves_repeated_layers_and_validates_each_descriptor() {
    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let credentials = application
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/accounts/{account}/containers/registries/registry.example.test/credentials"),
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"expiration_minutes":15,"permissions":["push","pull"]}))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(credentials.status(), StatusCode::OK);
    let credentials = body(credentials).await;
    let password = credentials["result"]["password"].as_str().unwrap();
    let auth = format!("Basic {}", STANDARD.encode(format!("{account}:{password}")));
    let repository = format!("{account}/repeated-layer");
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_mode(0o755);
    tar.append_data(&mut header, "app", b"main".as_slice())
        .unwrap();
    let layer = tar.into_inner().unwrap();
    let layer_digest = upload(&fixture, &application, &repository, &auth, &layer).await;
    let config = serde_json::to_vec(&json!({
        "os":"linux", "architecture":"arm64",
        "rootfs":{"type":"layers","diff_ids":[layer_digest, layer_digest]}
    }))
    .unwrap();
    let config_digest = upload(&fixture, &application, &repository, &auth, &config).await;
    let descriptor = json!({
        "mediaType":"application/vnd.oci.image.layer.v1.tar",
        "digest":layer_digest, "size":layer.len()
    });
    let manifest = json!({
        "schemaVersion":2, "mediaType":"application/vnd.oci.image.manifest.v1+json",
        "config":{"mediaType":"application/vnd.oci.image.config.v1+json",
            "digest":config_digest,"size":config.len()},
        "layers":[descriptor, descriptor]
    });
    let path = format!("/v2/{repository}/manifests/repeated");
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let mut conflicting = manifest.clone();
    conflicting["layers"][1]["size"] = json!(layer.len() + 1);
    let conflicting = serde_json::to_vec(&conflicting).unwrap();
    let rejected = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, conflicting.clone()))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    assert!(repo.registry_tag(account, &repository, "repeated").is_err());
    let rejected_digest: [u8; 32] = Sha256::digest(&conflicting).into();
    assert!(repo.image(account, &rejected_digest).is_err());
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let admitted = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, manifest.clone()))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::CREATED);
    let retrieved = application
        .oneshot(request(Method::GET, &path, &auth, Vec::new()))
        .await
        .unwrap();
    assert_eq!(retrieved.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(retrieved.into_body(), 1 << 20).await.unwrap(),
        manifest
    );
    let expected: [u8; 32] = Sha256::digest(&manifest).into();
    let metadata = repo.image(account, &expected).unwrap();
    let diff_id: [u8; 32] = Sha256::digest(&layer).into();
    assert_eq!(metadata.diff_ids, vec![diff_id, diff_id]);
    let artifact = ArtifactRef::new(
        ARTIFACT_KEY_VERSION,
        &hex::encode(metadata.artifact_sha256),
        metadata.artifact_size,
    )
    .unwrap();
    let pin = fixture
        .service
        .cache
        .acquire(&fixture.service.artifacts, &artifact)
        .await
        .unwrap();
    let image = open_compute_artifacts::verify_container_image_archive(
        pin.file(),
        &artifact,
        expected,
        "arm64",
        ContainerImageLimits {
            json_bytes: 8192,
            compressed_bytes: 8192,
            unpacked_bytes: 8192,
            layers: 8,
            entries: 8,
        },
    )
    .unwrap();
    assert_eq!(image.layers().len(), 2);
    assert_eq!(image.diff_ids(), &[diff_id, diff_id]);
}

#[tokio::test]
async fn container_registry_streams_verified_graphs_and_fences_auth_expiry_corruption_and_replay() {
    let fixture = fixture().await;
    let application = app(&fixture);
    let account = fixture.service.instance_id();
    let credentials_path =
        format!("/accounts/{account}/containers/registries/registry.example.test/credentials");
    let response = application
        .clone()
        .oneshot(request(
            Method::POST,
            &credentials_path,
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"expiration_minutes":15,"permissions":["push","pull"]}))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let credentials = body(response).await;
    let password = credentials["result"]["password"].as_str().unwrap();
    let auth = format!("Basic {}", STANDARD.encode(format!("{account}:{password}")));
    let handshake = application
        .clone()
        .oneshot(request(Method::GET, "/v2/", &auth, Vec::new()))
        .await
        .unwrap();
    assert_eq!(handshake.status(), StatusCode::OK);
    assert_eq!(
        handshake.headers()["docker-distribution-api-version"],
        "registry/2.0"
    );
    let repository = format!("{account}/app");
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_mode(0o755);
    tar.append_data(&mut header, "app", b"main".as_slice())
        .unwrap();
    let layer = tar.into_inner().unwrap();
    let layer_digest = upload(&fixture, &application, &repository, &auth, &layer).await;
    let config=serde_json::to_vec(&json!({"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[layer_digest]}})).unwrap();
    let config_digest = upload(&fixture, &application, &repository, &auth, &config).await;
    for method in [Method::HEAD, Method::GET] {
        let response = application
            .clone()
            .oneshot(request(
                method.clone(),
                &format!("/v2/{repository}/blobs/{layer_digest}"),
                &auth,
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            layer.len().to_string()
        );
        if method == Method::GET {
            assert_eq!(
                to_bytes(response.into_body(), 1 << 20).await.unwrap(),
                layer.as_slice()
            );
        }
    }
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":config_digest,"size":config.len()},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":layer_digest,"size":layer.len()}]})).unwrap();
    let path = format!("/v2/{repository}/manifests/release");
    let response = application
        .clone()
        .oneshot(request(Method::PUT, &path, &auth, manifest.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let digest = response.headers()["docker-content-digest"]
        .to_str()
        .unwrap()
        .to_owned();
    for path in [path, format!("/v2/{repository}/manifests/{digest}")] {
        let response = application
            .clone()
            .oneshot(request(Method::GET, &path, &auth, Vec::new()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1 << 20).await.unwrap(),
            manifest.as_slice()
        );
        let response = application
            .clone()
            .oneshot(request(Method::HEAD, &path, &auth, Vec::new()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for (path, authorization) in [
        ("/v2/".to_owned(), "Bearer wrong".to_owned()),
        (
            format!("/v2/another/app/blobs/{layer_digest}"),
            auth.clone(),
        ),
        (
            "/v2/".to_owned(),
            format!("Basic {}", STANDARD.encode("other:password")),
        ),
        ("/v2/".to_owned(), "Basic !!!!".to_owned()),
    ] {
        let response = application
            .clone()
            .oneshot(request(Method::GET, &path, &authorization, Vec::new()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::WWW_AUTHENTICATE],
            "Basic realm=\"Container Registry\""
        );
    }
    for path in [
        format!("/v2/{repository}/blobs/sha256:bad"),
        format!("/v2/{repository}/blobs/{layer_digest}?bad=1"),
        format!("/v2/{repository}/manifests/unknown"),
        format!("/v2/{repository}/unknown"),
    ] {
        let response = application
            .clone()
            .oneshot(request(Method::GET, &path, &auth, Vec::new()))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    let failed = application
        .clone()
        .oneshot(request(
            Method::PUT,
            &format!("/v2/{repository}/manifests/wrong"),
            &auth,
            b"{}".to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::BAD_REQUEST);
    let response = application
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/accounts/{account}/containers/me"),
            "Bearer read-token",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body(response).await["result"]["external_account_id"],
        account.to_string()
    );
    let expired = "e".repeat(64);
    ContainerAuthority::new(fixture.service.storage.db())
        .issue_registry_token(account, &password_hash(&expired), true, true, 0)
        .unwrap();
    let auth = format!("Basic {}", STANDARD.encode(format!("{account}:{expired}")));
    assert_eq!(
        application
            .oneshot(request(Method::GET, "/v2/", &auth, Vec::new()))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn container_management_preserves_immutable_targets_and_cursor_contracts() {
    use open_compute_core::{BindingKind, RequestId, ResourceId, VersionId};
    use open_compute_storage::{
        containers::VersionContainerDeclaration,
        durable_objects::{DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository},
        resources::{ReserveResourceCreate, ResourceCreateReservation, ResourceRepository},
        worker_repository::{
            EffectiveResourceLimits, NewVersion, NewVersionProducts, WorkerRepository,
        },
    };
    let fixture = fixture().await;
    let account = fixture.service.instance_id();
    let storage = &fixture.service.storage;
    let namespace = ResourceId::generate();
    let resources = ResourceRepository::new(storage.db());
    let reservation = resources
        .reserve_create(
            &ReserveResourceCreate {
                instance_id: account,
                kind: BindingKind::DoNamespace,
                name: "ContainerClass",
                idempotency_key: "namespace-create",
                fingerprint_key_id: "fixture",
                request_fingerprint: &[1; 32],
                resource_id: namespace,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 1,
                expires_at_ms: 1000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("namespace was not reserved")
    };
    DurableObjectRepository::new(storage)
        .ensure_namespace(&resource, fixture._artifacts.worker, "ContainerClass")
        .unwrap();
    resources.mark_ready(namespace, 2).unwrap();
    let version = VersionId::generate();
    let workers = WorkerRepository::new(storage.db());
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                instance_id: account,
                worker_id: fixture._artifacts.worker,
                content_kind: open_compute_storage::worker_repository::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                resource_limits: EffectiveResourceLimits::standard_defaults(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 3,
            },
            &NewVersionProducts {
                containers: &[
                    VersionContainerDeclaration::new(namespace, "invoice".into()).unwrap(),
                ],
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 4).unwrap();
    workers
        .promote(
            account,
            fixture._artifacts.worker,
            version,
            None,
            RequestId::generate(),
            5,
        )
        .unwrap();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(4);
    header.set_mode(0o755);
    tar.append_data(&mut header, "app", b"main".as_slice())
        .unwrap();
    let layer = tar.into_inner().unwrap();
    let layer_digest = hex::encode(Sha256::digest(&layer));
    let config=serde_json::to_vec(&json!({"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[format!("sha256:{layer_digest}")]}})).unwrap();
    let config_digest = hex::encode(Sha256::digest(&config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":format!("sha256:{config_digest}"),"size":config.len()},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":format!("sha256:{layer_digest}"),"size":layer.len()}]})).unwrap();
    let mut blobs = Vec::new();
    for bytes in [config, layer] {
        let sha = hex::encode(Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        blobs.push(
            fixture
                .service
                .artifacts
                .put_verified(
                    futures::stream::once(async {
                        Ok::<_, std::io::Error>(bytes::Bytes::from(bytes))
                    }),
                    &sha,
                    size,
                )
                .await
                .unwrap(),
        );
    }
    let sha: [u8; 32] = Sha256::digest(&manifest).into();
    fixture
        .service
        .admit_image(ContainerImageUpload {
            manifest: &manifest,
            expected: sha,
            architecture: "arm64",
            blobs,
            limits: ContainerImageLimits {
                json_bytes: 8192,
                compressed_bytes: 8192,
                unpacked_bytes: 8192,
                layers: 8,
                entries: 8,
            },
        })
        .await
        .unwrap();
    let image = format!(
        "registry.example.test/{account}/invoice@sha256:{}",
        hex::encode(sha)
    );
    let public_namespace = fixture
        .state
        .v4_instance_context()
        .unwrap()
        .public_resource_id(
            crate::cloudflare_v4::V4ResourceKind::DurableObjectNamespace,
            namespace,
        );
    let input = json!({"name":"invoice","scheduling_policy":"default","instances":0,"max_instances":4,"configuration":{"image":image,"instance_type":"lite"},"durable_objects":{"namespace_id":public_namespace},"constraints":{"tiers":[1,2]},"observability":{"logs":{"enabled":true}},"rollout_active_grace_period":0});
    let application = app(&fixture);
    let base = format!("/accounts/{account}/containers/applications");
    assert!(fixture.protocol.images.lock().await.is_empty());
    let imports = fixture
        .protocol
        .imports
        .load(std::sync::atomic::Ordering::SeqCst);
    let created = application
        .clone()
        .oneshot(request(
            Method::POST,
            &base,
            "Bearer deployer-token",
            serde_json::to_vec(&input).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(
        fixture
            .protocol
            .imports
            .load(std::sync::atomic::Ordering::SeqCst),
        imports
    );
    assert!(fixture.protocol.images.lock().await.is_empty());
    let result = body(created).await;
    let app_id = result["result"]["id"].as_str().unwrap();
    assert_eq!(result["result"]["configuration"]["memory_mib"], 256);
    assert_eq!(result["result"]["configuration"]["disk"]["size_mb"], 2000.0);
    assert_eq!(
        result["result"]["configuration"]["observability"]["logs"]["enabled"],
        true
    );
    assert_eq!(result["result"]["instances"], 0);
    let duplicate = application
        .clone()
        .oneshot(request(
            Method::POST,
            &base,
            "Bearer deployer-token",
            serde_json::to_vec(&input).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    for suffix in ["?per_page=0", "?page_token=invalid", "?page=1"] {
        let response = application
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{base}{suffix}"),
                "Bearer read-token",
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let listed = body(
        application
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{base}?name=invoice&per_page=1"),
                "Bearer read-token",
                Vec::new(),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["result"].as_array().unwrap().len(), 1);
    assert_eq!(listed["result_info"]["next_page_token"], "");
    let instances = body(
        application
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{base}/{app_id}/instances-v2?state=active"),
                "Bearer read-token",
                Vec::new(),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(instances["result"], json!([]));
    assert_eq!(instances["result_info"]["per_page"], 100);
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let metadata_only = body(
        application
            .clone()
            .oneshot(request(
                Method::PATCH,
                &format!("{base}/{app_id}"),
                "Bearer deployer-token",
                serde_json::to_vec(
                    &json!({"max_instances":3,"observability":{"logs":{"enabled":false}}}),
                )
                .unwrap(),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(metadata_only["result"]["max_instances"], 3);
    assert_eq!(metadata_only["result"]["version"], 1);
    assert_ne!(
        metadata_only["result"]["updated_at"],
        result["result"]["updated_at"]
    );
    assert_eq!(
        metadata_only["result"]["configuration"]["observability"]["logs"]["enabled"],
        false
    );
    let rollout_input = json!({"description":"Deploy invoice","strategy":"rolling","kind":"full_auto","target_configuration":input["configuration"],"steps":[{"description":"Replace all","step_size":{"percentage":100}}]});
    let rollout_path = format!("{base}/{app_id}/rollouts");
    let created = application
        .clone()
        .oneshot(request(
            Method::POST,
            &rollout_path,
            "Bearer deployer-token",
            serde_json::to_vec(&rollout_input).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let rollout = body(created).await;
    let rollout_id = rollout["result"]["id"].as_str().unwrap();
    assert_eq!(rollout["result"]["status"], "progressing");
    assert_eq!(rollout["result"]["description"], "Deploy invoice");
    assert_eq!(rollout["result"]["steps"][0]["description"], "Replace all");
    assert_eq!(rollout["result"]["progress"]["current_step"], 0);
    ContainerAuthority::new(storage.db())
        .rollout_step_leases(
            account,
            app_id.parse().unwrap(),
            rollout_id.parse().unwrap(),
            open_compute_core::wall_time_ms(),
        )
        .unwrap();
    let finished = body(
        application
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{rollout_path}/{rollout_id}"),
                "Bearer read-token",
                Vec::new(),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(finished["result"]["status"], "completed");
    assert_eq!(finished["result"]["progress"]["updated_instances"], 0);
    let removed = application
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("{base}/{app_id}"),
            "Bearer deployer-token",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    let gone = application
        .oneshot(request(
            Method::GET,
            &format!("{base}/{app_id}"),
            "Bearer read-token",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    // Run the repository-pinned producer against the same real SQLite/SigV4 authority.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let producer_router = Router::new().nest("/client/v4", app(&fixture));
    let server = tokio::spawn(async move { axum::serve(listener, producer_router).await.unwrap() });
    let origin = format!("http://{address}/client/v4");
    run_cf(
        &origin,
        account,
        &[
            "containers",
            "applications",
            "create",
            "--body",
            &input.to_string(),
        ],
    )
    .await;
    let created = ContainerAuthority::new(storage.db())
        .applications(account)
        .unwrap();
    assert_eq!(created.len(), 1);
    let created_id = created[0].id.to_string();
    run_cf(&origin, account, &["containers", "applications", "list"]).await;
    let replacement_configuration = serde_json::json!({"image":input["configuration"]["image"],"vcpu":0.03125,"memory_mib":128,"disk":{"size_mb":1000}});
    let edited = serde_json::json!({"max_instances":3,"configuration":replacement_configuration});
    let mut cli_rollout = rollout_input.clone();
    cli_rollout["target_configuration"] = replacement_configuration;
    run_cf(
        &origin,
        account,
        &[
            "containers",
            "applications",
            "edit",
            &created_id,
            "--body",
            &edited.to_string(),
        ],
    )
    .await;
    assert_eq!(
        ContainerAuthority::new(storage.db())
            .applications(account)
            .unwrap()[0]
            .max_instances,
        3
    );
    run_cf(
        &origin,
        account,
        &[
            "containers",
            "applications",
            "rollouts",
            "create",
            &created_id,
            "--body",
            &cli_rollout.to_string(),
        ],
    )
    .await;
    let persisted = ContainerAuthority::new(storage.db())
        .rollouts(account, created[0].id)
        .unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].target_generation, 2);
    assert_eq!(
        persisted[0].details.description.as_deref(),
        Some("Deploy invoice")
    );
    server.abort();
}

#[tokio::test]
async fn container_management_resolves_registry_tags_once_and_rejects_missing_tags() {
    let fixture = fixture().await;
    let original = fixture.admit_image("tag-original").await;
    let replacement = fixture.admit_image("tag-replacement").await;
    let authority = authority(&fixture.service, &fixture._artifacts, original, true);
    let instance = fixture.service.instance_id();
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    fixture
        .service
        .delete_application(authority.app.id)
        .await
        .unwrap();
    let repository = format!("{instance}/tagged");
    repo.publish_registry_tag(instance, &repository, "release", &original, 10)
        .unwrap();
    let public_namespace = fixture
        .state
        .v4_instance_context()
        .unwrap()
        .public_resource_id(
            crate::cloudflare_v4::V4ResourceKind::DurableObjectNamespace,
            authority.object.namespace_resource_id,
        );
    let image = format!("registry.example.test/{repository}:release");
    let missing = format!("registry.example.test/{repository}:missing");
    let normalized = format!(
        "registry.example.test/{repository}@sha256:{}",
        hex::encode(original)
    );
    let next_normalized = format!(
        "registry.example.test/{repository}@sha256:{}",
        hex::encode(replacement)
    );
    let mut input = json!({"name":authority.app.name,"scheduling_policy":"default","instances":0,"max_instances":1,
        "configuration":{"image":missing,"instance_type":"lite"},"durable_objects":{"namespace_id":public_namespace},
        "constraints":{"tiers":[1,2]},"observability":{"logs":{"enabled":false}},"rollout_active_grace_period":0});
    let application = app(&fixture);
    let base = format!("/accounts/{instance}/containers/applications");
    let missing_create = application
        .clone()
        .oneshot(request(
            Method::POST,
            &base,
            "Bearer deployer-token",
            serde_json::to_vec(&input).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_create.status(), StatusCode::NOT_FOUND);
    assert!(repo.applications(instance).unwrap().is_empty());
    input["configuration"]["image"] = image.clone().into();
    let created = application
        .clone()
        .oneshot(request(
            Method::POST,
            &base,
            "Bearer deployer-token",
            serde_json::to_vec(&input).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = body(created).await;
    assert_eq!(created["result"]["configuration"]["image"], normalized);
    let id: ResourceId = created["result"]["id"].as_str().unwrap().parse().unwrap();
    let path = format!("{base}/{id}");
    assert_eq!(
        repo.target(instance, id, 1).unwrap().spec.image_sha256,
        Some(original)
    );

    repo.publish_registry_tag(instance, &repository, "release", &replacement, 11)
        .unwrap();
    let unchanged = body(
        application
            .clone()
            .oneshot(request(Method::GET, &path, "Bearer read-token", Vec::new()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(unchanged["result"]["configuration"]["image"], normalized);
    assert_eq!(unchanged["result"]["version"], 1);
    let updated = application
        .clone()
        .oneshot(request(
            Method::PATCH,
            &path,
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"configuration":{"image":image,"instance_type":"lite"}}))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body(updated).await;
    assert_eq!(updated["result"]["configuration"]["image"], next_normalized);
    assert_eq!(updated["result"]["version"], 2);
    assert_eq!(
        repo.target(instance, id, 1).unwrap().spec.image_sha256,
        Some(original)
    );
    assert_eq!(
        repo.target(instance, id, 2).unwrap().spec.image_sha256,
        Some(replacement)
    );

    let missing_update = application
        .clone()
        .oneshot(request(
            Method::PATCH,
            &path,
            "Bearer deployer-token",
            serde_json::to_vec(&json!({"configuration":{"image":missing,"instance_type":"lite"}}))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_update.status(), StatusCode::NOT_FOUND);
    let retained = body(
        application
            .oneshot(request(Method::GET, &path, "Bearer read-token", Vec::new()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        retained["result"]["configuration"]["image"],
        next_normalized
    );
    assert_eq!(retained["result"]["version"], 2);
}

async fn run_cf(origin: &str, account: open_compute_core::InstanceId, args: &[&str]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let cli = root.join("node_modules/cf/bin/cf");
    assert!(cli.is_file(), "repository-pinned cf is required");
    let mut command = tokio::process::Command::new("node");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("CLOUDFLARE_API_BASE_URL", origin)
        .env("CLOUDFLARE_ACCOUNT_ID", account.to_string())
        .env("CLOUDFLARE_API_TOKEN", "deployer-token")
        .env("CI", "true")
        .env("NO_COLOR", "1")
        .current_dir(root)
        .arg(cli)
        .args(args)
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "pinned cf failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn control_credentials_fence_late_generations_unknown_results_and_cancellation() {
    use open_compute_runtime::GenerationAuthRegistry;
    use std::sync::atomic::Ordering;
    let first_fixture = fixture().await;
    let auth = Arc::new(GenerationAuthRegistry::new());
    auth.activate_for_test(open_compute_core::SecretString::new("generation-old"));
    let old = auth.credential().unwrap();
    first_fixture
        .credentials
        .block
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&first_fixture.service);
    let registry = Arc::clone(&auth);
    let first_old = old.clone();
    let first = tokio::spawn(async move {
        service
            .credentials_for_generation(&"a".repeat(64), &registry, &first_old)
            .await
    });
    first_fixture.credentials.entered.notified().await;
    auth.activate_for_test(open_compute_core::SecretString::new("generation-new"));
    let current = auth.credential().unwrap();
    let service = Arc::clone(&first_fixture.service);
    let registry = Arc::clone(&auth);
    let late = tokio::spawn(async move {
        service
            .credentials_for_generation(&"a".repeat(64), &registry, &old)
            .await
    });
    let service = Arc::clone(&first_fixture.service);
    let registry = Arc::clone(&auth);
    let newer = tokio::spawn(async move {
        service
            .credentials_for_generation(&"a".repeat(64), &registry, &current)
            .await
    });
    first_fixture.credentials.release.notify_one();
    assert!(first.await.unwrap().is_err());
    assert!(late.await.unwrap().is_err());
    assert!(newer.await.unwrap().is_ok());
    let generations = first_fixture.credentials.observed.lock().await;
    assert_eq!(generations.len(), 2);
    assert_ne!(generations[0], generations[1]);
    drop(generations);

    first_fixture.credentials.fail.store(true, Ordering::SeqCst);
    let current = auth.credential().unwrap();
    assert!(
        first_fixture
            .service
            .credentials_for_generation(&"a".repeat(64), &auth, &current)
            .await
            .is_err()
    );
    first_fixture
        .credentials
        .fail
        .store(false, Ordering::SeqCst);
    assert!(
        first_fixture
            .service
            .credentials_for_generation(&"a".repeat(64), &auth, &current)
            .await
            .is_err()
    );
    assert_eq!(first_fixture.credentials.observed.lock().await.len(), 3);
    assert!(
        first_fixture
            .service
            .credentials_for_generation(&"b".repeat(64), &auth, &current)
            .await
            .is_ok()
    );
    assert_eq!(first_fixture.credentials.observed.lock().await.len(), 4);

    let cancelled_fixture = fixture().await;
    cancelled_fixture
        .credentials
        .block
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&cancelled_fixture.service);
    let registry = Arc::clone(&auth);
    let token = current.clone();
    let cancelled = tokio::spawn(async move {
        service
            .credentials_for_generation(&"a".repeat(64), &registry, &token)
            .await
    });
    cancelled_fixture.credentials.entered.notified().await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    cancelled_fixture.credentials.release.notify_one();
    assert!(
        cancelled_fixture
            .service
            .credentials_for_generation(&"a".repeat(64), &auth, &current)
            .await
            .is_err()
    );
    assert_eq!(cancelled_fixture.credentials.observed.lock().await.len(), 1);
}

#[tokio::test]
async fn container_admission_checks_the_live_broker_after_qualification() {
    let mut fixture = fixture().await;
    fixture.service.ensure_active().await.unwrap();
    fixture.shutdown.notify_one();
    (&mut fixture.broker).await.unwrap();
    let error = fixture.service.ensure_active().await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
    assert_eq!(
        ContainerAuthority::new(fixture.service.storage.db())
            .applications(fixture.service.instance_id())
            .unwrap()
            .len(),
        0
    );
}

#[path = "container_registry_protocol_tests.rs"]
mod protocol;
