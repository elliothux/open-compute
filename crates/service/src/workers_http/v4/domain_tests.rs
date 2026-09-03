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
    let mut input = UploadInput::new(metadata).unwrap();
    input
        .apply_explicit_bindings(
            api,
            &AccountAuthority::new(PlatformId::generate(), account, 1),
            account,
            WorkerId::generate(),
            None,
            false,
        )
        .unwrap();

    let binding = input.bindings.get("EVENTS").unwrap();
    assert_eq!(binding.kind, BindingKind::QueueProducer);
    assert_eq!(binding.config, CanonicalBindingConfig::default());
    assert_eq!(queues.get(account, queue_id).unwrap().config, config);
}
