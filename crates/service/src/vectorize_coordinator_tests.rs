use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::config::{DataConfig, MetricsConfig};
use open_compute_core::{BindingKind, PlatformStatus, RequestId, SystemClock};
use open_compute_storage::{
    VECTORIZE_SCHEMA_VERSION, VectorMutationInput, VectorMutationKind, inspect_resources,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, ResourceController, ResourcePins,
    VectorizeIndexSpec, VectorizeResourceDriver,
};

#[tokio::test]
async fn coordinator_applies_one_durable_frontier_per_index() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let controller = ResourceController::new(
        &storage,
        ResourcePins::new(),
        VectorizeResourceDriver::new(
            &storage,
            VectorizeIndexSpec {
                dimensions: 32,
                metric: "cosine".to_string(),
                quota_vectors: 100,
                quota_bytes: 16 * 1024 * 1024,
            },
            5_000,
        ),
    );
    let resource = match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::VectorizeIndex,
            name: "coordinator".to_string(),
            idempotency_key: "coordinator-create".to_string(),
            driver_schema_version: VECTORIZE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 1,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first create replayed"),
    };
    let index = VectorizeIndexRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    let engine = open_engine(&storage, &index).unwrap();
    engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[VectorMutationInput {
                id: "a".to_string(),
                namespace: None,
                values: Some(
                    std::iter::once(1.0)
                        .chain(std::iter::repeat_n(0.0, 31))
                        .collect(),
                ),
                metadata: None,
            }],
            2,
        )
        .unwrap();
    assert!(engine.get_by_ids(&["a".to_string()]).unwrap().is_empty());
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let pins = ResourcePins::new();
    pins.fence_and_wait(resource, Duration::from_millis(100))
        .await
        .unwrap();
    let fenced = VectorizeCoordinator::new(storage.clone(), pins.clone())
        .drain_once()
        .unwrap();
    assert_eq!(fenced.applied, 0);
    assert!(engine.get_by_ids(&["a".to_string()]).unwrap().is_empty());
    pins.unfence(resource);
    ResourceRepository::new(storage.db())
        .set_availability(
            account,
            resource,
            ResourceAvailability::Unavailable,
            Some("TEST_UNAVAILABLE"),
            2,
        )
        .unwrap();
    let report = VectorizeCoordinator::new(storage.clone(), pins.clone())
        .with_metrics(metrics.clone())
        .drain_once()
        .unwrap();
    assert_eq!(report.indexes, 1);
    assert_eq!(report.applied, 1);
    assert_eq!(report.claimed, 0);
    assert_eq!(engine.get_by_ids(&["a".to_string()]).unwrap().len(), 1);
    assert_eq!(
        VectorizeIndexRepository::new(storage.db())
            .get(account, resource)
            .unwrap()
            .resource
            .availability,
        ResourceAvailability::Healthy
    );
    let rendered = metrics.render(&PlatformStatus::starting());
    assert!(rendered.contains("vectorize_coordinator_applied_total 1"));
    assert!(rendered.contains("vectorize_ready_indexes 1"));

    engine
        .enqueue(
            VectorMutationKind::Delete,
            &[VectorMutationInput {
                id: "a".to_owned(),
                namespace: None,
                values: None,
                metadata: None,
            }],
            3,
        )
        .unwrap();
    let now_ms = unix_ms();
    assert!(
        engine
            .claim_next("external-claim", now_ms, 60_000)
            .unwrap()
            .is_some()
    );
    let claimed = VectorizeCoordinator::new(storage.clone(), pins.clone())
        .drain_once()
        .unwrap();
    assert_eq!(claimed.indexes, 1);
    assert_eq!(claimed.applied, 0);
    assert_eq!(claimed.claimed, 1);
    assert!(
        engine
            .apply_claimed("external-claim", now_ms)
            .unwrap()
            .is_some()
    );
    drop(engine);

    let path = VectorizePaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&index.storage_key, account, resource)
        .unwrap();
    std::fs::write(path, b"corrupt").unwrap();
    let blocked = VectorizeCoordinator::new(storage.clone(), pins)
        .drain_once()
        .unwrap();
    assert_eq!(blocked.blocked, 1);
    assert_eq!(
        VectorizeIndexRepository::new(storage.db())
            .get(account, resource)
            .unwrap()
            .resource
            .availability,
        ResourceAvailability::Unavailable
    );
    drop(storage);
    let inspected = inspect_resources(&root.join("control.sqlite"), 5_000, 100).unwrap();
    assert_eq!(inspected.len(), 1);
    assert_eq!(inspected[0].kind, BindingKind::VectorizeIndex);
}
