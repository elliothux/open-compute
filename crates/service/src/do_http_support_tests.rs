use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use axum::body::to_bytes;
use open_compute_core::SystemClock;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{AuthorizedDurableObjectDelete, WorkerRepository};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;
use tower::ServiceExt as _;

pub(super) struct Fixture {
    pub(super) _temp: TempDir,
    pub(super) router: Router,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) api: DoApiState,
    pub(super) deletes: Arc<FakeDeleteTransport>,
    pub(super) account: AccountId,
    pub(super) worker: WorkerId,
}

#[derive(Debug, Default)]
pub(super) struct FakeDeleteTransport {
    pub(super) calls: Mutex<Vec<AuthorizedDurableObjectDelete>>,
    pub(super) fail_next: AtomicBool,
}

impl DurableObjectDeleteTransport for FakeDeleteTransport {
    fn delete<'a>(
        &'a self,
        authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(authority.clone());
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(PlatformError::new(
                    ErrorCode::DoStorageUnavailable,
                    "test Durable Object transport is unavailable",
                ));
            }
            Ok(())
        })
    }
}

pub(super) fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "durable-api", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0
        .id;
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let mut api = DoApiState::new(
        storage.clone(),
        ResourcePins::new(),
        transport,
        DurableObjectsConfig::default(),
        Duration::from_millis(50),
    )
    .with_metrics(metrics.clone());
    let deletes = Arc::new(FakeDeleteTransport::default());
    api.transport = deletes.clone();
    let state = HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
        .with_do_api(api.clone());
    Fixture {
        _temp: temp,
        router: http::admin_router(state),
        storage,
        api,
        deletes,
        account,
        worker,
    }
}

pub(super) fn request(
    method: &str,
    uri: &str,
    body: impl Into<Value>,
    key: Option<&str>,
) -> Request {
    let body = body.into();
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_HEADER, key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

pub(super) async fn json_response(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

pub(super) async fn create_namespace_fixture(
    fixture: &Fixture,
    name: &str,
    class_name: &str,
) -> ResourceId {
    let collection = format!(
        "/v1/accounts/{}/durable-objects/namespaces",
        fixture.account
    );
    let (status, body) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({
                    "name": name,
                    "workerId": fixture.worker,
                    "className": class_name
                }),
                Some(&format!("create-{name}")),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["resourceId"].as_str().unwrap().parse().unwrap()
}

pub(super) fn object_id(namespace: ResourceId, fill: u8) -> DurableObjectId {
    let mut bytes = [fill; 32];
    bytes[..8].copy_from_slice(&open_compute_core::durable_object_namespace_prefix(
        namespace,
    ));
    DurableObjectId::for_namespace(bytes, namespace).unwrap()
}

pub(super) fn insert_object(
    storage: &PlatformStorage,
    namespace: ResourceId,
    object: DurableObjectId,
    state: DurableObjectState,
    now_ms: i64,
) {
    let connection =
        rusqlite::Connection::open(storage.data_dir().root().join("control.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO do_objects(namespace_resource_id, object_id, generation, state, \
             created_at_ms, updated_at_ms, deleted_at_ms) VALUES (?1, ?2, 1, ?3, ?4, ?4, NULL)",
            rusqlite::params![
                namespace.to_string(),
                object.to_string(),
                state.as_str(),
                now_ms
            ],
        )
        .unwrap();
}
