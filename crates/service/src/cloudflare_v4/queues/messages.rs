use super::{authority, context, platform_error, resolve_queue};
use crate::cloudflare_v4::{V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::post;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::{
    QueueAvailability, QueueContentType, QueueEnqueueRequest, QueueMessageInput, QueueMetrics,
    QueueState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/queues/{queue_id}/messages",
            post(push),
        )
        .route(
            "/accounts/{account_id}/queues/{queue_id}/messages/batch",
            post(bulk_push),
        )
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    #[serde(default)]
    body: Option<Value>,
    content_type: Option<MessageContentType>,
    delay_seconds: Option<u32>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MessageContentType {
    Json,
    Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchBody {
    delay_seconds: Option<u32>,
    #[serde(default)]
    messages: Vec<MessageBody>,
}

#[derive(Serialize)]
struct PushResult {
    metadata: PushMetadata,
}

#[derive(Serialize)]
struct PushMetadata {
    metrics: PushMetrics,
}

#[derive(Serialize)]
struct PushMetrics {
    backlog_bytes: u64,
    backlog_count: u64,
    oldest_message_timestamp_ms: i64,
}

async fn push(state: State<HttpState>, path: Path<(String, String)>, request: Request) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match body::<MessageBody>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    enqueue(state, path, context, None, vec![body]).await
}

async fn bulk_push(
    state: State<HttpState>,
    path: Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let body = match body::<BatchBody>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    enqueue(state, path, context, body.delay_seconds, body.messages).await
}

async fn enqueue(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    context: crate::cloudflare_v4::V4RequestContext,
    batch_delay_seconds: Option<u32>,
    messages: Vec<MessageBody>,
) -> Response {
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let queue = match resolve_queue(authority, api.storage(), account_id, &queue_public) {
        Ok(value)
            if value.state == QueueState::Ready
                && value.availability == QueueAvailability::Healthy =>
        {
            value
        }
        Ok(_) => {
            return platform_error(
                &PlatformError::new(ErrorCode::QueueNotReady, "Queue is not ready"),
                context,
            );
        }
        Err(error) => return platform_error(&error, context),
    };
    let messages = match messages
        .into_iter()
        .map(message)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    let scheduler = api.scheduler().clone();
    let request_id = context.request_id().as_uuid();
    let task = tokio::task::spawn_blocking(move || {
        scheduler.enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: queue.id,
                request_id,
                output_gate: false,
                lifecycle_generation: queue.lifecycle_generation,
                config_generation: queue.config_generation,
                batch_delay_seconds,
                messages,
            },
            open_compute_core::wall_time_ms(),
        )
    });
    match tokio::time::timeout(TIMEOUT, task).await {
        Ok(Ok(Ok(result))) => success_response(context, push_result(result.metrics)),
        Ok(Ok(Err(error))) => platform_error(&error, context),
        Ok(Err(_)) => error_response(V4Error::Internal, context.request_id()),
        Err(_) => platform_error(
            &PlatformError::new(
                ErrorCode::QueueSendResultUnknown,
                "Queue send result is unknown",
            ),
            context,
        ),
    }
}

async fn body<T: serde::de::DeserializeOwned>(
    request: Request,
    request_id: open_compute_core::RequestId,
) -> Result<T, Response> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some("application/json") {
        return Err(error_response(V4Error::InvalidRequest, request_id));
    }
    let bytes = to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, request_id))?;
    serde_json::from_slice(&bytes).map_err(|_| error_response(V4Error::InvalidRequest, request_id))
}

fn message(value: MessageBody) -> Result<QueueMessageInput, PlatformError> {
    let content_type = value.content_type.unwrap_or_else(|| {
        if value.body.as_ref().is_some_and(Value::is_string) {
            MessageContentType::Text
        } else {
            MessageContentType::Json
        }
    });
    let (content_type, body) = match content_type {
        MessageContentType::Text => (
            QueueContentType::Text,
            match value.body.as_ref() {
                Some(Value::String(body)) => body.as_bytes().to_vec(),
                None => Vec::new(),
                Some(_) => return Err(invalid_message()),
            },
        ),
        MessageContentType::Json => (
            QueueContentType::Json,
            serde_json::to_vec(&value.body.unwrap_or(Value::Null))
                .map_err(|_| invalid_message())?,
        ),
    };
    Ok(QueueMessageInput {
        content_type,
        body,
        delay_seconds: value.delay_seconds,
    })
}

fn push_result(metrics: QueueMetrics) -> PushResult {
    PushResult {
        metadata: PushMetadata {
            metrics: PushMetrics {
                backlog_bytes: metrics.backlog_bytes,
                backlog_count: metrics.backlog_count,
                oldest_message_timestamp_ms: metrics.oldest_message_timestamp_ms.unwrap_or(0),
            },
        },
    }
}

fn invalid_message() -> PlatformError {
    PlatformError::new(ErrorCode::QueueInvalidMessage, "Queue message is invalid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use open_compute_core::{
        DeterministicSchedulerClock, QueueId, SchedulerConfig, SecretString, WorkflowsConfig,
    };
    use open_compute_runtime::GenerationAuthRegistry;
    use open_compute_storage::{QueueConfig, QueueProjection, QueueRepository, SchedulerStore};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt as _;

    #[test]
    fn official_message_union_infers_and_validates_content_types() {
        let text = message(serde_json::from_str(r#"{"body":"hello"}"#).unwrap()).unwrap();
        assert_eq!(text.content_type, QueueContentType::Text);
        assert_eq!(text.body, b"hello");

        let json = message(serde_json::from_str(r#"{"body":{"ok":true}}"#).unwrap()).unwrap();
        assert_eq!(json.content_type, QueueContentType::Json);
        assert_eq!(json.body, br#"{"ok":true}"#);

        let empty = message(serde_json::from_str(r#"{"content_type":"text"}"#).unwrap()).unwrap();
        assert!(empty.body.is_empty());
        assert!(
            message(serde_json::from_str(r#"{"body":7,"content_type":"text"}"#).unwrap()).is_err()
        );
        assert!(serde_json::from_str::<MessageBody>(r#"{"body":"x","extra":true}"#).is_err());
    }

    #[tokio::test]
    async fn push_and_bulk_push_are_durable() {
        let (_temp, _mock, state, account, storage) =
            crate::tests::initialized_worker_http_fixture().await;
        let scheduler_store = Arc::new(
            SchedulerStore::open(
                &storage.data_dir().ensure_scheduler_db().unwrap(),
                100,
                1,
                account,
            )
            .unwrap(),
        );
        let queue = QueueId::generate();
        let config = QueueConfig::default();
        QueueRepository::new(storage.db())
            .insert_creating(account, queue, "messages", config, 1)
            .unwrap();
        scheduler_store
            .create_queue_projection(&QueueProjection {
                queue_id: queue,
                instance_id: account,
                lifecycle_generation: 1,
                config_generation: 1,
                config,
                created_at_ms: 1,
                updated_at_ms: 1,
            })
            .unwrap();
        QueueRepository::new(storage.db())
            .mark_ready(account, queue, 2)
            .unwrap();
        let not_ready = QueueId::generate();
        QueueRepository::new(storage.db())
            .insert_creating(account, not_ready, "not-ready", config, 3)
            .unwrap();
        let scheduler = Arc::new(crate::SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            crate::runtime_bridge::WorkerdTransport::new(
                GenerationAuthRegistry::new(),
                Arc::new(Mutex::new(None)),
            ),
            SchedulerConfig::default(),
            WorkflowsConfig::default(),
            Arc::new(DeterministicSchedulerClock::new(10)),
        ));
        let api = crate::QueueApiState::new(storage.clone(), scheduler.clone(), 8);
        let authority = crate::cloudflare_v4::accounts::V4InstanceContext::new(account, 1);
        let base = format!(
            "/client/v4/accounts/{}/queues/{}/messages",
            authority.public_id(),
            authority.public_queue_id(queue),
        );
        let not_ready_path = format!(
            "/client/v4/accounts/{}/queues/{}/messages",
            authority.public_id(),
            authority.public_queue_id(not_ready),
        );
        let app = crate::http::admin_router(
            state
                .with_queue_api(Some(api))
                .with_scheduler(Some(scheduler))
                .with_platform_storage(storage)
                .with_v4_tokens(
                    SecretString::new("deployer-token"),
                    SecretString::new("read-token"),
                )
                .with_v4_instance_context(authority),
        );
        assert_eq!(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(not_ready_path)
                        .header(header::AUTHORIZATION, "Bearer deployer-token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"body":"blocked"}"#))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        for request in [
            Request::builder()
                .method("POST")
                .uri(&base)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"body":"unauthorized"}"#))
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(&base)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::from(r#"{"body":"missing type"}"#))
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(&base)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("not-json"))
                .unwrap(),
            Request::builder()
                .method("POST")
                .uri(&base)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"body":7,"content_type":"text"}"#))
                .unwrap(),
        ] {
            assert!(
                app.clone()
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status()
                    .is_client_error()
            );
        }
        for _ in 0..2 {
            let request = Request::builder()
                .method("POST")
                .uri(&base)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"body":"one"}"#))
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::OK
            );
        }
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{base}/batch"))
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"body":{"n":2}},{"body":"three"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            scheduler_store
                .queue_metrics(queue, 1, 1)
                .unwrap()
                .backlog_count,
            4
        );
    }
}
