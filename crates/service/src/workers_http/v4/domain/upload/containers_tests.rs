use super::*;
use crate::containers::test_support::{Fixture, authority, fixture};
use crate::runtime_bridge::WorkerdTransport;
use crate::workers_http::v4::multipart::parse_worker_upload;
use axum::body::Body;
use axum::extract::{FromRequest as _, Multipart};
use axum::http::{Request, header};
use open_compute_workers::{BundleLimits, CanonicalBundle, ModuleInput, ModuleType, VersionPins};
use serde_json::{Value, json};
use std::time::Duration;

fn metadata(class: &str) -> WorkerUploadMetadata {
    serde_json::from_value(json!({"main_module":"index.js","compatibility_date":"2026-09-08",
        "containers":[{"name":"authority-app","class_name":class}],
        "exports":{(class):{"type":"durable-object","state":"created","storage":"sqlite","container":"authority-app"}}})).unwrap()
}
fn api(fixture: &Fixture) -> WorkerApiState {
    WorkerApiState::new(
        Arc::clone(&fixture.service.storage),
        fixture.service.artifacts.clone(),
        WorkerdTransport::for_test_endpoint(std::path::Path::new("/tmp/missing.sock")),
        VersionPins::new(),
        BundleLimits::default(),
        Duration::from_millis(10),
    )
    .with_containers(Some(Arc::clone(&fixture.service)))
}

#[tokio::test]
async fn container_upload_projection_requires_matching_same_worker_sqlite_export() {
    let fixture = fixture().await;
    let image = fixture.admit_image("main").await;
    let authority = authority(&fixture.service, &fixture._artifacts, image, true);
    let second = fixture.admit_image("alternate").await;
    let api = api(&fixture);
    let instance = fixture.service.instance_id();
    let mut input = UploadInput::new(metadata("ContainerClass")).unwrap();
    input
        .apply_containers(&api, instance, fixture._artifacts.worker, None)
        .unwrap();
    assert_eq!(input.runtime_features.containers.len(), 1);
    assert_eq!(
        input.runtime_features.containers[0].namespace_resource_id,
        authority.object.namespace_resource_id
    );
    assert_eq!(
        input.runtime_features.containers[0].application_name,
        "authority-app"
    );
    input.runtime_features.containers[0].validate().unwrap();
    let mut named = metadata("ContainerClass");
    named.containers[0].images = BTreeMap::from([
        (
            "blue".into(),
            format!("registry/app@sha256:{}", hex::encode(image)),
        ),
        (
            "green".into(),
            format!("registry/app@sha256:{}", hex::encode(second)),
        ),
    ]);
    let mut named_input = UploadInput::new(named.clone()).unwrap();
    named_input
        .apply_containers(&api, instance, fixture._artifacts.worker, None)
        .unwrap();
    assert_eq!(
        named_input.runtime_features.containers[0].images,
        named.containers[0].images
    );
    assert_ne!(
        named_input.runtime_features.containers[0].descriptor_sha256,
        input.runtime_features.containers[0].descriptor_sha256
    );
    named.containers[0].images.insert(
        "missing".into(),
        format!("registry/app@sha256:{}", "9".repeat(64)),
    );
    assert!(
        UploadInput::new(named)
            .unwrap()
            .apply_containers(&api, instance, fixture._artifacts.worker, None)
            .is_err()
    );

    assert!(
        UploadInput::new(metadata("ContainerClass"))
            .unwrap()
            .apply_containers(&api, instance, WorkerId::generate(), None)
            .is_err()
    );
    assert!(
        UploadInput::new(metadata("MissingClass"))
            .unwrap()
            .apply_containers(&api, instance, fixture._artifacts.worker, None)
            .is_err()
    );
    for value in [
        json!({"storage":"kv","state":"created","container":"authority-app","type":"durable-object"}),
        json!({"storage":"sqlite","state":"deleted","container":"authority-app","type":"durable-object"}),
        json!({"storage":"sqlite","state":"created","container":"different-app","type":"durable-object"}),
        json!({"type":"worker"}),
    ] {
        let mut invalid = metadata("ContainerClass");
        invalid.exports = Some(serde_json::from_value(json!({"ContainerClass":value})).unwrap());
        assert!(
            UploadInput::new(invalid)
                .unwrap()
                .apply_containers(&api, instance, fixture._artifacts.worker, None)
                .is_err()
        );
    }
    let empty: WorkerUploadMetadata =
        serde_json::from_value(json!({"main_module":"index.js","compatibility_date":"2026-09-08"}))
            .unwrap();
    let mut empty = UploadInput::new(empty).unwrap();
    empty
        .apply_containers(&api, instance, fixture._artifacts.worker, None)
        .unwrap();
    assert!(empty.runtime_features.containers.is_empty());
}

#[tokio::test]
async fn container_upload_rejects_provider_loss_before_any_durable_object_migration() {
    let mut fixture = fixture().await;
    let api = api(&fixture);
    let instance = fixture.service.instance_id();
    api.require_containers().await.unwrap();
    let workers = WorkerRepository::new(fixture.service.storage.db());
    let worker = workers
        .get_worker(instance, fixture._artifacts.worker)
        .unwrap();
    let before = DurableObjectRepository::new(&fixture.service.storage)
        .list_namespaces(instance)
        .unwrap()
        .len();
    let metadata = metadata("NewContainerClass");
    do_lifecycle::validate_containers(&metadata).unwrap();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export class NewContainerClass {} export default {fetch(){return new Response('ok')}}".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap()
    .into_bytes();
    fixture.broker.abort();
    assert!((&mut fixture.broker).await.unwrap_err().is_cancelled());
    std::fs::remove_file(fixture._socket.path().join("broker.sock")).unwrap();
    let error = create_from_upload(
        &api,
        &V4InstanceContext::new(instance, 0),
        instance,
        &worker,
        ParsedWorkerUpload {
            metadata,
            bundle: Some(bundle),
        },
        false,
        None,
        None,
        RequestId::generate(),
        10,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable, "{error:?}");
    assert_eq!(
        DurableObjectRepository::new(&fixture.service.storage)
            .list_namespaces(instance)
            .unwrap()
            .len(),
        before
    );
}

async fn container_multipart(metadata: Value) -> ParsedWorkerUpload {
    let boundary = "container-declarative-multipart";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport class NewContainerClass {{}} export default {{fetch(){{return new Response('ok')}}}}\r\n--{boundary}--\r\n"
    );
    let request = Request::builder()
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let multipart = Multipart::from_request(request, &()).await.unwrap();
    parse_worker_upload(multipart, BundleLimits::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn container_cf_declarative_multipart_prepares_sqlite_without_legacy_migrations() {
    let fixture = fixture().await;
    let api = api(&fixture);
    let instance = fixture.service.instance_id();
    let worker = fixture._artifacts.worker;
    let repo = DurableObjectRepository::new(&fixture.service.storage);
    let before = repo.list_namespaces(instance).unwrap().len();
    let declaration = json!({
        "main_module":"index.js", "compatibility_date":"2026-09-08",
        "bindings":[{"type":"durable_object_namespace","name":"OBJECTS","class_name":"NewContainerClass"}],
        "containers":[{"name":"authority-app","class_name":"NewContainerClass"}],
        "exports":{"default":{"type":"worker"},"ContainerProxy":{"type":"worker"},"NewContainerClass":{"type":"durable-object","state":"created","storage":"sqlite","container":"authority-app"}}
    });
    let mut ambiguous = declaration.clone();
    ambiguous["migrations"] =
        json!({"new_tag":"v1","steps":[{"new_sqlite_classes":["NewContainerClass"]}]});
    let ambiguous = container_multipart(ambiguous).await;
    assert_eq!(
        do_lifecycle::prepare(
            &api,
            instance,
            worker,
            &ambiguous.metadata,
            ambiguous.bundle.as_deref(),
            10
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::BundleInvalid
    );
    assert_eq!(repo.list_namespaces(instance).unwrap().len(), before);
    let upload = container_multipart(declaration).await;
    assert!(upload.metadata.migrations.is_none());
    let migration = do_lifecycle::prepare(
        &api,
        instance,
        worker,
        &upload.metadata,
        upload.bundle.as_deref(),
        11,
    )
    .unwrap()
    .unwrap();
    let pending = repo
        .namespace_for_worker_upload(instance, worker, "NewContainerClass", Some(migration.tag()))
        .unwrap();
    assert_eq!(repo.list_namespaces(instance).unwrap().len(), before);
    assert!(
        UploadInput::new(upload.metadata.clone())
            .unwrap()
            .apply_containers(&api, instance, worker, None)
            .is_err()
    );
    assert!(
        UploadInput::new(upload.metadata.clone())
            .unwrap()
            .apply_containers(&api, instance, worker, Some("wrong-migration"))
            .is_err()
    );
    assert!(
        UploadInput::new(upload.metadata.clone())
            .unwrap()
            .apply_containers(&api, instance, WorkerId::generate(), Some(migration.tag()))
            .is_err()
    );
    let mut input = UploadInput::new(upload.metadata).unwrap();
    input
        .apply_containers(&api, instance, worker, Some(migration.tag()))
        .unwrap();
    assert_eq!(input.runtime_features.containers.len(), 1);
    let association = &input.runtime_features.containers[0];
    association.validate().unwrap();
    assert_eq!(association.application_name, "authority-app");
    assert_eq!(association.namespace_resource_id, pending.resource.id);
    assert_eq!(pending.owner_worker_id, worker);
    assert_eq!(pending.class_name, "NewContainerClass");
    migration.rollback(&api, worker, 12).unwrap();
    assert!(
        repo.namespace_for_worker_upload(
            instance,
            worker,
            "NewContainerClass",
            Some(migration.tag())
        )
        .is_err()
    );
}

#[test]
fn container_upload_named_image_limits_use_canonical_unicode_and_count_contract() {
    let reference = format!("app@sha256:{}", "1".repeat(64));
    let mut input = metadata("ContainerClass");
    input.containers[0].images = (0..100)
        .map(|index| (format!("image-{index}"), reference.clone()))
        .collect();
    do_lifecycle::validate_containers(&input).unwrap();
    input.containers[0]
        .images
        .insert("extra".into(), reference.clone());
    assert_eq!(
        do_lifecycle::validate_containers(&input)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    for name in ["a".repeat(128), "🦀".repeat(128)] {
        input.containers[0].images = BTreeMap::from([(name, reference.clone())]);
        do_lifecycle::validate_containers(&input).unwrap();
    }
    for name in [String::new(), "a".repeat(129), "🦀".repeat(129)] {
        input.containers[0].images = BTreeMap::from([(name, reference.clone())]);
        assert_eq!(
            do_lifecycle::validate_containers(&input)
                .unwrap_err()
                .code(),
            ErrorCode::BundleInvalid
        );
    }
}
