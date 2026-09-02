//! OpenAI-compatible provider contract against an owned loopback fixture.

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, Response, StatusCode};
use axum::routing::post;
use open_compute_core::{
    AiAuthConfig, AiConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiGenerationCapability,
    AiGenerationModelConfig, AiProviderConfig, AiTokenizer, AiTokenizerArtifactConfig,
};
use open_compute_service::ai_provider::{
    AiProviderError, ChatMessage, OpenAiChatClient, OpenAiProviderClient,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

const ALIAS: &str = "@cf/qwen/qwen3-embedding-0.6b";

#[derive(Clone, Copy)]
enum FixtureResponse {
    Success,
    Malformed,
    Unauthorized,
    RateLimited,
    ServerError,
    Delayed,
}

async fn fixture(State(response): State<FixtureResponse>, body: Bytes) -> Response<String> {
    let valid_request = serde_json::from_slice::<serde_json::Value>(&body).is_ok_and(|value| {
        value.get("model") == Some(&json!(ALIAS))
            && value.get("input") == Some(&json!(["fixture input"]))
            && value.get("encoding_format") == Some(&json!("float"))
    });
    if !valid_request {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(String::new())
            .expect("response");
    }
    if matches!(response, FixtureResponse::Delayed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (status, body) = match response {
        FixtureResponse::Success | FixtureResponse::Delayed => (
            StatusCode::OK,
            json!({
                "object": "list",
                "model": ALIAS,
                "data": [{
                    "object": "embedding",
                    "index": 0,
                    "embedding": vec![0.25_f32; 1024]
                }],
                "usage": {"prompt_tokens": 3, "total_tokens": 3}
            })
            .to_string(),
        ),
        FixtureResponse::Malformed => (
            StatusCode::OK,
            json!({
                "object": "list",
                "model": ALIAS,
                "data": [{"object": "embedding", "index": 1, "embedding": [1.0]}]
            })
            .to_string(),
        ),
        FixtureResponse::Unauthorized => (StatusCode::UNAUTHORIZED, "credential data".into()),
        FixtureResponse::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "provider detail".into()),
        FixtureResponse::ServerError => {
            (StatusCode::INTERNAL_SERVER_ERROR, "provider detail".into())
        }
    };
    let mut result = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body)
        .expect("response");
    if matches!(response, FixtureResponse::RateLimited) {
        result
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("7"));
    }
    result
}

async fn chat_fixture(body: Bytes) -> Response<String> {
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(String::new())
                .expect("response");
        }
    };
    if request.get("model") != Some(&json!("fixture-chat")) {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(String::new())
            .expect("response");
    }
    if request.get("stream") == Some(&json!(true)) {
        return Response::builder()
            .header("content-type", "text/event-stream")
            .body(
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hel\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_owned(),
            )
            .expect("response");
    }
    let system = request
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let content = if system.starts_with("Rewrite") {
        "rewritten query"
    } else if system.starts_with("Rank") {
        "[1,0]"
    } else {
        "answer"
    };
    Response::builder()
        .header("content-type", "application/json")
        .body(
            json!({
                "model": "fixture-chat",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": content},
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
        )
        .expect("response")
}

async fn client(
    response: FixtureResponse,
    timeout_ms: u64,
) -> (OpenAiProviderClient, AiConfig, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let port = listener.local_addr().expect("address").port();
    let app = Router::new()
        .route("/v1/embeddings", post(fixture))
        .route("/v1/chat/completions", post(chat_fixture))
        .with_state(response);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let mut config = AiConfig {
        provider_timeout_ms: timeout_ms,
        query_timeout_ms: timeout_ms,
        ..AiConfig::default()
    };
    config.providers.insert(
        "fixture".into(),
        AiProviderConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            auth: AiAuthConfig::None,
        },
    );
    config.embedding_models.insert(
        ALIAS.into(),
        AiEmbeddingModelConfig {
            provider: "fixture".into(),
            remote_model: ALIAS.into(),
            model_revision: "fixture-revision".into(),
            dimensions: 1024,
            request_dimensions: None,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 8192,
            tokenizer: AiTokenizer::Qwen3,
            tokenizer_revision: "fixture-tokenizer-revision".into(),
            tokenizer_artifact: AiTokenizerArtifactConfig {
                path: "/opt/open-compute/models/qwen3/tokenizer.json".into(),
                sha256: "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a".into(),
            },
        },
    );
    config.default_embedding_model = Some(ALIAS.into());
    config.generation_models.insert(
        "fixture/generation".into(),
        AiGenerationModelConfig {
            provider: "fixture".into(),
            remote_model: "fixture-chat".into(),
            model_revision: "fixture-chat-revision".into(),
            max_context_tokens: 4_096,
            capabilities: BTreeSet::from([
                AiGenerationCapability::Chat,
                AiGenerationCapability::Rewrite,
                AiGenerationCapability::Rerank,
            ]),
        },
    );
    let contract = config.resolve_embedding_model(None).expect("contract");
    (
        OpenAiProviderClient::new(&config, &contract).expect("client"),
        config,
        task,
    )
}

#[tokio::test]
async fn loopback_contract_classifies_success_and_provider_failures() {
    let input = vec!["fixture input".to_owned()];
    let (success, config, task) = client(FixtureResponse::Success, 1_000).await;
    let batch = success.embeddings(&input).await.expect("embedding");
    assert_eq!(batch.embeddings.len(), 1);
    assert_eq!(batch.embeddings[0].len(), 1024);
    assert_eq!(batch.prompt_tokens, Some(3));
    assert!(!success.contract_sha256().is_empty());

    let chat = OpenAiChatClient::new(&config, "fixture/generation", AiGenerationCapability::Chat)
        .expect("chat client");
    let completion = chat
        .chat(&[ChatMessage::user("hello")], 64)
        .await
        .expect("chat");
    assert_eq!(completion.content, "answer");
    let mut stream = chat
        .chat_stream(&[ChatMessage::user("hello")], 64)
        .await
        .expect("stream");
    assert_eq!(stream.next_delta().await, Ok(Some("hel".into())));
    assert_eq!(stream.next_delta().await, Ok(Some("lo".into())));
    assert_eq!(stream.next_delta().await, Ok(None));
    assert!(stream.is_done());
    let rewrite = OpenAiChatClient::new(
        &config,
        "fixture/generation",
        AiGenerationCapability::Rewrite,
    )
    .expect("rewrite client");
    assert_eq!(
        rewrite.rewrite_query("original").await,
        Ok("rewritten query".into())
    );
    let rerank = OpenAiChatClient::new(
        &config,
        "fixture/generation",
        AiGenerationCapability::Rerank,
    )
    .expect("rerank client");
    assert_eq!(
        rerank
            .rerank("query", &["first".into(), "second".into()])
            .await,
        Ok(vec![1, 0])
    );
    task.abort();

    for (fixture_response, expected) in [
        (
            FixtureResponse::Malformed,
            AiProviderError::MalformedResponse,
        ),
        (FixtureResponse::Unauthorized, AiProviderError::Unauthorized),
        (
            FixtureResponse::RateLimited,
            AiProviderError::RateLimited {
                retry_after_seconds: Some(7),
            },
        ),
        (FixtureResponse::ServerError, AiProviderError::Transient),
        (FixtureResponse::Delayed, AiProviderError::Timeout),
    ] {
        let timeout = if matches!(fixture_response, FixtureResponse::Delayed) {
            10
        } else {
            1_000
        };
        let (client, _, task) = client(fixture_response, timeout).await;
        assert_eq!(client.embeddings(&input).await, Err(expected));
        task.abort();
    }
}
