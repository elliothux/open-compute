//! Real SQLite/SigV4 authority and official cf producer contracts for named images.

use super::test_support::snapshots::rootfs;
use super::{test_support::*, *};
use crate::runtime_bridge::WorkerdTransport;
use crate::workers_http::WorkerApiState;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use open_compute_storage::containers::{ContainerSchedulingPolicy, ContainerTargetSpec};
use open_compute_storage::worker_repository::WorkerRepository;
use open_compute_workers::{BundleLimits, VersionPins};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tower::ServiceExt as _;

fn router(fixture: &Fixture) -> Router {
    let api = WorkerApiState::new(
        Arc::clone(&fixture.service.storage),
        fixture.service.artifacts.clone(),
        WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock")),
        VersionPins::new(),
        BundleLimits::default(),
        std::time::Duration::from_millis(10),
    );
    let state = fixture.state.clone().with_worker_api(api);
    Router::new()
        .nest(
            "/client/v4",
            crate::cloudflare_v4::router(
                state.clone(),
                crate::cloudflare_v4::storage_router().merge(crate::workers_http::v4::router()),
            ),
        )
        .with_state(state)
}

async fn request(
    router: &Router,
    method: Method,
    path: &str,
    token: &str,
    value: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn named_images_preparation_requires_sealed_ordinary_artifacts_and_provider_bounds() {
    let fixture = fixture().await;
    let image = fixture.admit_image("named").await;
    let router = router(&fixture);
    let path = format!(
        "/client/v4/accounts/{}/containers/image-preparations",
        fixture.service.instance_id()
    );
    let reference = format!("registry.example.test/app@sha256:{}", hex::encode(image));
    let (status, value) = request(
        &router,
        Method::POST,
        &path,
        "Bearer deployer-token",
        json!({"image":reference}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["result"], json!({"status":"ready"}));
    assert_eq!(
        fixture
            .protocol
            .imports
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(fixture.protocol.registrations.lock().await.is_empty());
    for (body, expected) in [
        (
            json!({"image":"registry/app:latest"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"image":format!("registry/app@sha256:{}","9".repeat(64))}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"image":reference,"extra":true}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            request(&router, Method::POST, &path, "Bearer deployer-token", body)
                .await
                .0,
            expected
        );
    }
    assert_eq!(
        request(
            &router,
            Method::POST,
            &path,
            "Bearer read-token",
            json!({"image":reference})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let mut wrong_arch = repo.image(fixture.service.instance_id(), &image).unwrap();
    wrong_arch.manifest_sha256 = [7; 32];
    wrong_arch.architecture = "amd64".into();
    repo.admit_image(fixture.service.instance_id(), &wrong_arch)
        .unwrap();
    assert_eq!(request(&router,Method::POST,&path,"Bearer deployer-token",json!({"image":format!("registry/app@sha256:{}",hex::encode(wrong_arch.manifest_sha256))})).await.0,StatusCode::BAD_REQUEST);
    let mut absent_artifact = wrong_arch;
    absent_artifact.manifest_sha256 = [8; 32];
    absent_artifact.architecture = "arm64".into();
    absent_artifact.artifact_sha256 = [8; 32];
    repo.admit_image(fixture.service.instance_id(), &absent_artifact)
        .unwrap();
    assert!(
        fixture
            .service
            .prepare_image(&format!(
                "registry/app@sha256:{}",
                hex::encode(absent_artifact.manifest_sha256)
            ))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn durable_object_scheduling_preserves_named_version_images_and_requires_explicit_start_image()
 {
    let fixture = fixture().await;
    let blue = fixture.admit_image("blue").await;
    let green = fixture.admit_image("green").await;
    let reference = |image| format!("registry.example.test/app@sha256:{}", hex::encode(image));
    let images = BTreeMap::from([
        ("blue".into(), reference(blue)),
        ("绿色".into(), reference(green)),
    ]);
    let (declaration, version, route) =
        declared_namespace(&fixture.service, &fixture._artifacts, images.clone());
    let namespace = declaration.namespace_resource_id;
    let instance = fixture.service.instance_id();
    let public_namespace = fixture
        .state
        .v4_instance_context()
        .unwrap()
        .public_resource_id(
            crate::cloudflare_v4::V4ResourceKind::DurableObjectNamespace,
            namespace,
        );
    let router = router(&fixture);
    let version_path = format!(
        "/client/v4/accounts/{instance}/workers/scripts/{}/versions/{version}",
        WorkerRepository::new(fixture.service.storage.db())
            .get_worker(instance, fixture._artifacts.worker)
            .unwrap()
            .name
    );
    let (status, projected) = request(
        &router,
        Method::GET,
        &version_path,
        "Bearer read-token",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        projected["result"]["resources"]["script_runtime"]["containers"],
        json!([{"name":"authority-app","class_name":"ContainerClass","images":images}])
    );
    let base = format!("/client/v4/accounts/{instance}/containers/applications");
    let input = json!({"name":"authority-app","scheduling_policy":"durable_object","durable_objects":{"namespace_id":public_namespace}});
    let default_input = json!({"name":"authority-app","durable_objects":{"namespace_id":public_namespace},"configuration":{"image":reference(blue),"instance_type":"lite"},"max_instances":1});
    assert_eq!(
        request(
            &router,
            Method::POST,
            &base,
            "Bearer deployer-token",
            default_input
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        ContainerAuthority::new(fixture.service.storage.db())
            .applications(instance)
            .unwrap()
            .is_empty()
    );
    let mut mixed = input.clone();
    mixed["configuration"] = json!({"image":reference(blue),"instance_type":"lite"});
    assert_eq!(
        request(&router, Method::POST, &base, "Bearer deployer-token", mixed)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, created) =
        request(&router, Method::POST, &base, "Bearer deployer-token", input).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["result"]["id"], public_namespace);
    assert_eq!(
        created["result"]["durable_objects"]["namespace_id"],
        public_namespace
    );
    assert_eq!(created["result"]["scheduling_policy"], "durable_object");
    assert!(created["result"].get("configuration").is_none());
    let public_path = format!("{base}/{public_namespace}");
    assert_eq!(
        request(
            &router,
            Method::GET,
            &public_path,
            "Bearer read-token",
            Value::Null
        )
        .await
        .1["result"]["id"],
        public_namespace
    );
    let (status, patched) = request(
        &router,
        Method::PATCH,
        &public_path,
        "Bearer deployer-token",
        json!({"observability":{"logs":{"enabled":true}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["result"]["observability"]["logs"]["enabled"], true);
    assert!(patched["result"].get("configuration").is_none());
    assert_eq!(
        request(
            &router,
            Method::PATCH,
            &public_path,
            "Bearer deployer-token",
            json!({"max_instances":2})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let repo = ContainerAuthority::new(fixture.service.storage.db());
    let app = repo
        .application_for_namespace(instance, namespace, "authority-app")
        .unwrap();
    assert_eq!(
        app.scheduling_policy,
        ContainerSchedulingPolicy::DurableObject
    );
    let target = repo.target(instance, app.id, 1).unwrap();
    assert_eq!(target.spec.image_sha256, None);
    assert_eq!(repo.application_settings(instance, app.id).unwrap().0, None);
    assert_eq!(
        repo.target_configuration(instance, app.id, 1).unwrap(),
        None
    );
    assert_eq!(target.spec.resources, fixture.service.provider.resources());
    let mut owner = Authority {
        app,
        object: ContainerObjectAuthority {
            instance_id: instance,
            application_id: namespace,
            namespace_resource_id: namespace,
            object_id: insert_object(&fixture.service, namespace, 71),
            object_generation: 1,
            route_generation: route,
            descriptor_sha256: declaration.descriptor_sha256,
        },
    };

    assert!(
        fixture
            .service
            .resolve(owner.request(true, None), 10, "container-unit-generation")
            .await
            .is_err()
    );
    let first = fixture
        .service
        .resolve(
            owner.request(
                true,
                Some(json!({"image":reference(blue),"instanceProfile":"lite"})),
            ),
            11,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    fixture.running().await;
    fixture
        .service
        .resolve(owner.request(false, None), 12, "container-unit-generation")
        .await
        .unwrap();
    let (status, instances) = request(
        &router,
        Method::GET,
        &format!("{public_path}/instances-v2"),
        "Bearer read-token",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(instances["result"][0]["application_id"], public_namespace);
    assert_eq!(instances["result"][0]["image"], reference(blue));
    assert_eq!(instances["result"][0]["status"]["state"], "running");
    assert!(
        fixture
            .service
            .resolve(
                owner.request(true, Some(json!({"image":reference(green)}))),
                13,
                "container-unit-generation"
            )
            .await
            .is_err()
    );
    let restarted = fixture.restart_service();
    let resumed = restarted
        .resolve(owner.request(false, None), 14, "container-unit-generation")
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(resumed.opaque_engine_key, first.opaque_engine_key);
    assert_eq!(
        repo.existing_lease(&owner.object)
            .unwrap()
            .unwrap()
            .effective_image_sha256,
        blue
    );
    *fixture.protocol.snapshot_rootfs.lock().await = rootfs(b"named checkpoint");
    let captured = fixture
        .service
        .capture_snapshot(SnapshotRequest {
            directory: None,
            source: owner.stop(&first),
            name: Some("blue".into()),
        })
        .await
        .unwrap();
    let saved = repo
        .snapshots(instance)
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == captured.snapshot.id)
        .unwrap();
    assert_eq!(saved.source_image_sha256, blue);
    assert!(
        fixture
            .service
            .prepare_image(&reference(saved.image_sha256.unwrap()))
            .await
            .is_err()
    );
    fixture.service.stop(&owner.stop(&first), 15).await.unwrap();
    owner.object.object_id = insert_object(&fixture.service, namespace, 72);
    let second = restarted
        .resolve(
            owner.request(true, Some(json!({"image":reference(green)}))),
            16,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(
        repo.existing_lease(&owner.object)
            .unwrap()
            .unwrap()
            .effective_image_sha256,
        green
    );
    fixture
        .service
        .stop(&owner.stop(&second), 17)
        .await
        .unwrap();
    owner.object.object_id = insert_object(&fixture.service, namespace, 73);
    let restored = restarted
        .resolve(
            owner.request(true, Some(json!({"containerSnapshotId":saved.id}))),
            18,
            "container-unit-generation",
        )
        .await
        .unwrap()
        .lease
        .unwrap();
    assert_eq!(
        repo.existing_lease(&owner.object)
            .unwrap()
            .unwrap()
            .effective_image_sha256,
        saved.image_sha256.unwrap()
    );
    restarted.stop(&owner.stop(&restored), 19).await.unwrap();
    assert!(
        repo.application_leases(instance, namespace)
            .unwrap()
            .iter()
            .all(|lease| lease.state == ContainerLeaseState::Stopped)
    );
    assert_eq!(
        repo.version_declarations(instance, version).unwrap()[0].images,
        images
    );
    let references = WorkerRepository::new(fixture.service.storage.db())
        .referenced_artifacts()
        .unwrap();
    for image in [blue, green] {
        let image = repo.image(instance, &image).unwrap();
        assert!(references.contains(&(image.artifact_sha256, image.artifact_size)));
    }
    let standard = ContainerTargetSpec {
        image_sha256: Some(blue),
        ..target.spec
    };
    assert!(
        repo.create_durable_object_application(instance, &owner.app, &standard, false, 18)
            .is_err()
    );
}
