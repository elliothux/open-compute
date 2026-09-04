use super::*;
use open_compute_core::{
    BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, PlatformId, QueueId,
    ResourceId, SecretBytes, VersionId,
};
use open_compute_storage::{
    BuiltinBindingKind, NewQueueProducerBinding, NewVersion, NewVersionBinding, NewVersionProducts,
    NewVersionService, QueueConfig, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, StoredVersionSecret, VersionBuiltinBindingRecord, VersionContentKind,
};
use open_compute_workers::{BuiltinBindingDescriptorKindV1, BuiltinBindingDescriptorV1};

fn ready_resource(
    api: &WorkerApiState,
    account: AccountId,
    kind: BindingKind,
    name: &str,
) -> ResourceId {
    let fingerprint = api.storage.crypto().fingerprint_request(name.as_bytes());
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(api.storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind,
                name,
                idempotency_key: name,
                fingerprint_key_id: api.storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 1,
                expires_at_ms: i64::MAX,
            },
            100,
        )
        .unwrap()
    else {
        panic!("expected resource reservation");
    };
    ResourceRepository::new(api.storage.db())
        .mark_ready(resource.id, 2)
        .unwrap();
    resource.id
}

#[tokio::test]
async fn deprecated_queue_binding_delay_does_not_change_queue_authority() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let queue_id = QueueId::generate();
    let queues = QueueRepository::new(api.storage.db());
    let config = QueueConfig {
        delivery_delay_seconds: 17,
        ..QueueConfig::default()
    };
    queues
        .insert_creating(account, queue_id, "events", config, 1)
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2026-08-30",
        "bindings": [{
            "name": "EVENTS",
            "type": "queue",
            "queue_name": "events",
            "delivery_delay": 60
        }]
    }))
    .unwrap();
    let mut input = UploadInput::new(metadata);
    input
        .apply_explicit_bindings(
            api,
            &AccountAuthority::new(PlatformId::generate(), account, 1),
            account,
            WorkerId::generate(),
            None,
            false,
            false,
            None,
            0,
        )
        .unwrap();

    let binding = input.bindings.get("EVENTS").unwrap();
    assert_eq!(binding.kind, BindingKind::QueueProducer);
    assert_eq!(binding.config, CanonicalBindingConfig::default());
    assert_eq!(queues.get(account, queue_id).unwrap().config, config);
}

#[tokio::test]
async fn service_binding_props_are_projected_into_the_immutable_version_input() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let target = WorkerRepository::new(api.storage.db())
        .create_worker(account, "catalog", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let props = serde_json::json!({
        "constructor": {"enabled": true},
        "nested": [1, {"__proto__": "ordinary JSON data"}],
    });
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2026-08-30",
        "bindings": [{
            "name": "CATALOG",
            "type": "service",
            "service": "catalog",
            "props": props.clone(),
        }]
    }))
    .unwrap();
    let mut input = UploadInput::new(metadata);
    input
        .apply_explicit_bindings(
            api,
            &AccountAuthority::new(PlatformId::generate(), account, 1),
            account,
            WorkerId::generate(),
            None,
            false,
            false,
            None,
            0,
        )
        .unwrap();

    let service = input.services.get("CATALOG").unwrap();
    assert_eq!(service.target_worker_id, target.id);
    assert_eq!(service.props, Some(props));
}

#[tokio::test]
async fn failed_upload_content_releases_its_unconsumed_workflow_reservation() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2026-08-30",
        "bindings": [{
            "name": "FLOW",
            "type": "workflow",
            "workflow_name": "failed-upload",
            "class_name": "Flow"
        }]
    }))
    .unwrap();
    let mut input = UploadInput::new(metadata);
    input
        .apply_explicit_bindings(
            api,
            &AccountAuthority::new(PlatformId::generate(), account, 1),
            account,
            WorkerId::generate(),
            None,
            false,
            true,
            Some("failed-operation"),
            1,
        )
        .unwrap();
    assert!(
        input
            .content(api, account, "caller", None, Some("failed-operation"), 2)
            .await
            .is_err()
    );
    input
        .release_workflow_reservations(api, account, 3)
        .unwrap();
    assert!(
        WorkflowRepository::new(api.storage.db())
            .definitions(
                account,
                Some("failed-upload"),
                None,
                CatalogSort::Name,
                CatalogDirection::Asc,
                None,
                10,
            )
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_binding_projection_accepts_every_day1_binding_kind() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let authority = AccountAuthority::new(PlatformId::generate(), account, 1);
    let worker = WorkerId::generate();
    let kv = ready_resource(api, account, BindingKind::KvNamespace, "kv-resource");
    let d1 = ready_resource(api, account, BindingKind::D1Database, "d1-resource");
    for (kind, name) in [
        (BindingKind::R2Bucket, "r2-resource"),
        (BindingKind::VectorizeIndex, "vector-resource"),
        (BindingKind::AiSearchNamespace, "search-namespace"),
        (BindingKind::AiSearchInstance, "search-instance"),
    ] {
        ready_resource(api, account, kind, name);
    }
    let target = WorkerRepository::new(api.storage.db())
        .create_worker(
            account,
            "service-target",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap()
        .0;
    let queue_id = QueueId::generate();
    let queues = QueueRepository::new(api.storage.db());
    queues
        .insert_creating(account, queue_id, "events", QueueConfig::default(), 1)
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module": "index.js",
        "compatibility_date": "2026-08-30",
        "annotations": {"workers/tag":"release"},
        "bindings": [
            {"name":"PLAIN","type":"plain_text","text":"value"},
            {"name":"JSON","type":"json","json":{"ok":true}},
            {"name":"SECRET","type":"secret_text","text":"secret"},
            {"name":"KV","type":"kv_namespace","namespace_id":authority.public_resource_id(V4ResourceKind::KvNamespace, kv)},
            {"name":"D1","type":"d1","id":authority.public_resource_id(V4ResourceKind::D1Database, d1)},
            {"name":"R2","type":"r2_bucket","bucket_name":"r2-resource"},
            {"name":"VECTOR","type":"vectorize","index_name":"vector-resource"},
            {"name":"SEARCH_NS","type":"ai_search_namespace","namespace":"search-namespace"},
            {"name":"SEARCH","type":"ai_search","instance_name":"search-instance"},
            {"name":"AI","type":"ai"},
            {"name":"IMAGES","type":"images"},
            {"name":"VERSION","type":"version_metadata"},
            {"name":"QUEUE","type":"queue","queue_name":"events"},
            {"name":"FLOW","type":"workflow","workflow_name":"flow","class_name":"Flow"},
            {"name":"SERVICE","type":"service","service":"service-target","entrypoint":"named","props":{"answer":42}},
            {"name":"ASSETS","type":"assets"},
            {"name":"WASM","type":"wasm_module","part":"module.wasm"},
            {"name":"TEXT","type":"text_blob","part":"text.txt"},
            {"name":"DATA","type":"data_blob","part":"data.bin"},
            {"name":"IGNORED","type":"inherit"}
        ]
    }))
    .unwrap();
    let mut input = UploadInput::new(metadata);
    input
        .apply_explicit_bindings(
            api,
            &authority,
            account,
            worker,
            None,
            false,
            true,
            Some("reservation"),
            3,
        )
        .unwrap();
    assert_eq!(input.vars.len(), 2);
    assert_eq!(input.secrets.len(), 1);
    assert_eq!(input.bindings.len(), 8);
    assert_eq!(input.services["SERVICE"].target_worker_id, target.id);
    assert!(input.runtime_features.ai.is_some());
    assert!(input.runtime_features.images.is_some());
    assert_eq!(input.runtime_features.module_bindings.len(), 3);
    input
        .release_workflow_reservations(api, account, 4)
        .unwrap();
}

#[tokio::test]
async fn explicit_binding_projection_rejects_cross_script_and_missing_resources() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let authority = AccountAuthority::new(PlatformId::generate(), account, 1);
    for binding in [
        serde_json::json!({"name":"DO","type":"durable_object_namespace","class_name":"State","script_name":"other"}),
        serde_json::json!({"name":"FLOW","type":"workflow","workflow_name":"flow","class_name":"Flow","script_name":"other"}),
        serde_json::json!({"name":"KV","type":"kv_namespace","namespace_id":"missing"}),
        serde_json::json!({"name":"QUEUE","type":"queue","queue_name":"missing"}),
        serde_json::json!({"name":"SERVICE","type":"service","service":"missing"}),
    ] {
        let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
            "main_module": "index.js",
            "compatibility_date": "2026-08-30",
            "bindings": [binding]
        }))
        .unwrap();
        let mut input = UploadInput::new(metadata);
        assert!(
            input
                .apply_explicit_bindings(
                    api,
                    &authority,
                    account,
                    WorkerId::generate(),
                    None,
                    false,
                    false,
                    None,
                    1,
                )
                .is_err()
        );
    }
}

#[tokio::test]
async fn strict_inheritance_restores_each_persisted_binding_family() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let api = state.worker_api().unwrap();
    let repository = WorkerRepository::new(storage.db());
    let source = repository
        .create_worker(account, "inherit-source", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let target = repository
        .create_worker(account, "inherit-target", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let resource_bindings = [
        ("KV", BindingKind::KvNamespace, "inherit-kv"),
        ("D1", BindingKind::D1Database, "inherit-d1"),
        ("DO", BindingKind::DoNamespace, "InheritDo"),
        ("R2", BindingKind::R2Bucket, "inherit-r2"),
        ("VECTOR", BindingKind::VectorizeIndex, "inherit-vector"),
        (
            "SEARCH_NAMESPACE",
            BindingKind::AiSearchNamespace,
            "inherit-search-namespace",
        ),
        ("SEARCH", BindingKind::AiSearchInstance, "inherit-search"),
    ];
    let queue_id = QueueId::generate();
    QueueRepository::new(storage.db())
        .insert_creating(
            account,
            queue_id,
            "inherit-queue",
            QueueConfig::default(),
            1,
        )
        .unwrap();
    QueueRepository::new(storage.db())
        .mark_ready(account, queue_id, 2)
        .unwrap();

    let version = VersionId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"secret".to_vec()),
            account,
            source.id,
            version,
            "SECRET",
            &revision,
        )
        .unwrap();
    let bindings = resource_bindings
        .iter()
        .enumerate()
        .map(|(index, (name, kind, resource_name))| NewVersionBinding {
            id: BindingId::generate(),
            name: (*name).to_owned(),
            kind: *kind,
            resource_id: ready_resource(api, account, *kind, resource_name),
            resource_spec_generation: 1,
            capability_version: 1,
            permissions_json: serde_json::to_vec(&CanonicalPermissions::default()).unwrap(),
            config_json: serde_json::to_vec(&CanonicalBindingConfig::default()).unwrap(),
            descriptor_sha256: [u8::try_from(index + 1).unwrap(); 32],
        })
        .collect::<Vec<_>>();
    let queue = NewQueueProducerBinding {
        id: BindingId::generate(),
        name: "QUEUE".to_owned(),
        queue_id,
        queue_lifecycle_generation: 1,
        capability_version: 1,
        descriptor_sha256: [2; 32],
    };
    let descriptor = ServiceDescriptorV1::new(
        "SERVICE".to_owned(),
        target.id,
        Some("named".to_owned()),
        None,
    )
    .unwrap();
    let service = NewVersionService {
        binding_name: "SERVICE".to_owned(),
        target_worker_id: target.id,
        entrypoint: Some("named".to_owned()),
        props_json: None,
        descriptor_sha256: descriptor.sha256().unwrap(),
    };
    let mut builtins = [
        (
            "AI",
            BuiltinBindingKind::Ai,
            BuiltinBindingDescriptorKindV1::Ai,
            None,
        ),
        (
            "IMAGES",
            BuiltinBindingKind::Images,
            BuiltinBindingDescriptorKindV1::Images,
            None,
        ),
        (
            "VERSION",
            BuiltinBindingKind::VersionMetadata,
            BuiltinBindingDescriptorKindV1::VersionMetadata,
            Some("release".to_owned()),
        ),
        (
            "WASM",
            BuiltinBindingKind::WasmModule,
            BuiltinBindingDescriptorKindV1::WasmModule,
            Some("module.wasm".to_owned()),
        ),
        (
            "TEXT",
            BuiltinBindingKind::TextBlob,
            BuiltinBindingDescriptorKindV1::TextBlob,
            Some("text.txt".to_owned()),
        ),
        (
            "DATA",
            BuiltinBindingKind::DataBlob,
            BuiltinBindingDescriptorKindV1::DataBlob,
            Some("data.bin".to_owned()),
        ),
    ]
    .map(|(name, kind, descriptor_kind, tag)| {
        let descriptor =
            BuiltinBindingDescriptorV1::new(name.to_owned(), descriptor_kind, tag.clone()).unwrap();
        VersionBuiltinBindingRecord {
            name: name.to_owned(),
            kind,
            tag,
            descriptor_sha256: descriptor.sha256().unwrap(),
        }
    });
    builtins.sort_by(|left, right| left.name.cmp(&right.name));
    let input = NewVersion {
        id: version,
        account_id: account,
        worker_id: source.id,
        content_kind: VersionContentKind::Worker,
        artifact_sha256: Some([4; 32]),
        artifact_size: Some(1),
        artifact_schema_version: Some(1),
        main_module: Some("index.js".to_owned()),
        worker_code_sha256: [5; 32],
        compatibility_date: "2026-08-30".to_owned(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::from([
            ("PLAIN".to_owned(), br#""value""#.to_vec()),
            ("JSON".to_owned(), br#"{"ok":true}"#.to_vec()),
        ]),
        secrets: BTreeMap::from([(
            "SECRET".to_owned(),
            StoredVersionSecret {
                name: "SECRET".to_owned(),
                revision_id: revision,
                envelope,
            },
        )]),
        request_id: RequestId::generate(),
        now_ms: 3,
    };
    repository
        .insert_staging_version(
            &input,
            &NewVersionProducts {
                bindings: &bindings,
                queue_bindings: std::slice::from_ref(&queue),
                services: std::slice::from_ref(&service),
                builtin_bindings: &builtins,
                ..Default::default()
            },
            100,
        )
        .unwrap();
    repository.begin_validation(version).unwrap();
    repository.mark_ready(version, 4).unwrap();
    let previous = repository
        .version_snapshot(account, source.id, version, false)
        .unwrap();
    let public = crate::workers_http::v4::projection::public_bindings(
        api,
        &AccountAuthority::new(PlatformId::generate(), account, 1),
        &previous,
    )
    .unwrap();
    assert_eq!(public.len(), 18);
    assert!(public.iter().all(|value| value.get("name").is_some()));

    let names = [
        "PLAIN",
        "JSON",
        "SECRET",
        "KV",
        "D1",
        "DO",
        "R2",
        "VECTOR",
        "SEARCH_NAMESPACE",
        "SEARCH",
        "QUEUE",
        "SERVICE",
        "AI",
        "IMAGES",
        "VERSION",
        "WASM",
        "TEXT",
        "DATA",
    ];
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module":"next.js",
        "compatibility_date":"2026-08-30",
        "bindings": names.map(|name| serde_json::json!({"name":name,"type":"inherit"}))
    }))
    .unwrap();
    let mut inherited = UploadInput::new(metadata);
    inherited
        .apply_inheritance(api, Some(&previous), true)
        .unwrap();
    assert_eq!(inherited.vars.len(), 2);
    assert_eq!(inherited.secrets.len(), 1);
    assert_eq!(inherited.bindings.len(), 8);
    assert_eq!(inherited.services.len(), 1);
    assert_eq!(inherited.runtime_features.module_bindings.len(), 3);
    assert!(inherited.runtime_features.ai.is_some());
    assert!(inherited.runtime_features.images.is_some());
    assert!(inherited.runtime_features.version_metadata.is_some());

    for (strict, previous, bindings) in [
        (false, Some(&previous), serde_json::json!([])),
        (
            true,
            None,
            serde_json::json!([{"name":"MISSING","type":"inherit"}]),
        ),
        (
            true,
            Some(&previous),
            serde_json::json!([{"name":"MISSING","type":"inherit"}]),
        ),
    ] {
        let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
            "main_module":"next.js",
            "compatibility_date":"2026-08-30",
            "keep_bindings": if strict { Vec::<String>::new() } else { vec!["json".to_owned()] },
            "bindings":bindings
        }))
        .unwrap();
        assert!(
            UploadInput::new(metadata)
                .apply_inheritance(api, previous, strict)
                .is_err()
        );
    }
}
