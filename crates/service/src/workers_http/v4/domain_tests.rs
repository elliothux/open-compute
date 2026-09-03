use super::*;
use open_compute_core::{PlatformId, QueueId};
use open_compute_storage::QueueConfig;

#[tokio::test]
async fn deprecated_queue_binding_delay_does_not_change_queue_authority() {
    let (_temp, _mock, state, account) = crate::tests::initialized_worker_http_fixture().await;
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
    let (_temp, _mock, state, account) = crate::tests::initialized_worker_http_fixture().await;
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
    let (_temp, _mock, state, account) = crate::tests::initialized_worker_http_fixture().await;
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
