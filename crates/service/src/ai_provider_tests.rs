use super::*;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming as HyperIncoming;
use hyper::header::{CONTENT_TYPE, RETRY_AFTER};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request as HyperRequest, Response, StatusCode};
use hyper_util::rt::TokioIo;
use open_compute_core::{
    AiAuthConfig, AiBackendConfig, AiBackendProtocol, AiConfig, AiEmbeddingModelConfig,
    AiEmbeddingProfileConfig, AiGenerationCapability, AiGenerationModelConfig, AiTokenizer,
    AiTokenizerArtifactConfig, AiTokenizerConfig, SecretReference,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

type ScriptedResponse = (StatusCode, &'static str, Vec<u8>);

#[derive(Clone)]
struct ScriptedServer {
    responses: Arc<Mutex<Vec<ScriptedResponse>>>,
}

impl ScriptedServer {
    fn new(responses: Vec<ScriptedResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }

    async fn serve(self) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let responses = self.responses;
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let responses = responses.clone();
                let _ = http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(move |_req: HyperRequest<HyperIncoming>| {
                            let responses = responses.clone();
                            async move {
                                let (status, content_type, body) =
                                    responses.lock().unwrap().pop().unwrap_or((
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        "text/plain",
                                        b"gone".to_vec(),
                                    ));
                                Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(status)
                                        .header(CONTENT_TYPE, content_type)
                                        .header(RETRY_AFTER, "7")
                                        .body(Full::new(Bytes::from(body)))
                                        .unwrap(),
                                )
                            }
                        }),
                    )
                    .await;
            }
        });
        port
    }
}

struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn capture_one(body: Vec<u8>) -> (u16, oneshot::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(stream);
        let _ = http1::Builder::new()
            .serve_connection(
                io,
                service_fn(move |request: HyperRequest<HyperIncoming>| {
                    let sender = sender.clone();
                    let body = body.clone();
                    async move {
                        let (parts, incoming) = request.into_parts();
                        let request_body = incoming.collect().await.unwrap().to_bytes().to_vec();
                        if let Some(sender) = sender.lock().unwrap().take() {
                            let _ = sender.send(CapturedRequest {
                                path: parts.uri.path().to_owned(),
                                headers: parts.headers,
                                body: request_body,
                            });
                        }
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    }
                }),
            )
            .await;
    });
    (port, receiver)
}

fn tokenizer_fixture() -> (PathBuf, String) {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer-word-level.json");
    let bytes = fs::read(&path).unwrap();
    (path, hex::encode(Sha256::digest(bytes)))
}

fn embedding_config(root: &str) -> AiConfig {
    let (path, sha256) = tokenizer_fixture();
    let mut config = AiConfig {
        provider_timeout_ms: 2_000,
        query_timeout_ms: 2_000,
        max_embedding_inputs_per_batch: 8,
        ..AiConfig::default()
    };
    config.backends.insert(
        "fixture-embeddings".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiEmbeddingsV1,
            endpoint: format!("{root}/embeddings"),
            auth: AiAuthConfig::None,
            headers: Default::default(),
        },
    );
    let alias = "@cf/qwen/qwen3-embedding-0.6b";
    let profile = "fixture/qwen3-1024";
    config.embedding_profiles.insert(
        profile.to_owned(),
        AiEmbeddingProfileConfig {
            dimensions: 1_024,
            max_input_tokens: 8_192,
            send_dimensions: false,
            tokenizer: AiTokenizerConfig {
                kind: AiTokenizer::Qwen3,
                revision: "fixture-tokenizer".to_owned(),
                artifact: AiTokenizerArtifactConfig { path, sha256 },
            },
        },
    );
    config.embedding_models.insert(
        alias.to_owned(),
        AiEmbeddingModelConfig {
            backend: "fixture-embeddings".to_owned(),
            remote_model: alias.to_owned(),
            provider_revision: Some("fixture-model".to_owned()),
            profile: profile.to_owned(),
        },
    );
    config.default_embedding_model = Some(alias.to_owned());
    config
}

fn chat_config(root: &str) -> AiConfig {
    let mut config = embedding_config(root);
    config.backends.insert(
        "fixture-chat".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiChatCompletionsV1,
            endpoint: format!("{root}/chat/completions"),
            auth: AiAuthConfig::None,
            headers: Default::default(),
        },
    );
    let mut capabilities = BTreeSet::new();
    capabilities.insert(AiGenerationCapability::Chat);
    capabilities.insert(AiGenerationCapability::Rewrite);
    capabilities.insert(AiGenerationCapability::Rerank);
    config.generation_models.insert(
        "fixture/chat".to_owned(),
        AiGenerationModelConfig {
            backend: "fixture-chat".to_owned(),
            remote_model: "fixture-chat".to_owned(),
            provider_revision: Some("rev-1".to_owned()),
            max_context_tokens: 4_096,
            capabilities,
        },
    );
    config.default_generation_model = Some("fixture/chat".to_owned());
    config
}

fn embedding_ok_body(model: &str, dims: usize) -> Vec<u8> {
    let values = vec![0.125_f32; dims];
    serde_json::to_vec(&serde_json::json!({
        "object": "list",
        "model": model,
        "data": [{
            "object": "embedding",
            "index": 0,
            "embedding": values,
        }],
        "usage": {"prompt_tokens": 3, "total_tokens": 3}
    }))
    .unwrap()
}

fn chat_ok_body(model: &str, content: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }]
    }))
    .unwrap()
}

#[test]
fn error_display_and_message_constructors_cover_all_variants() {
    let variants = [
        AiProviderError::InvalidRequest,
        AiProviderError::ContractMismatch,
        AiProviderError::Unauthorized,
        AiProviderError::RateLimited {
            retry_after_seconds: Some(3),
        },
        AiProviderError::Transient,
        AiProviderError::Permanent,
        AiProviderError::Timeout,
        AiProviderError::MalformedResponse,
    ];
    for variant in variants {
        let text = variant.to_string();
        assert!(!text.is_empty());
        let _ = format!("{variant:?}");
    }
    assert_eq!(ChatMessage::system("s").content, "s");
    assert_eq!(ChatMessage::user("u").content, "u");
    assert_eq!(ChatMessage::assistant("a").content, "a");
}

#[tokio::test]
async fn embeddings_success_and_request_validation() {
    let model = "@cf/qwen/qwen3-embedding-0.6b";
    let body = embedding_ok_body(model, 1_024);
    let port = ScriptedServer::new(vec![(StatusCode::OK, "application/json", body)])
        .serve()
        .await;
    let config = embedding_config(&format!("http://127.0.0.1:{port}/v1"));
    let contract = config.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&config, &contract).unwrap();
    assert_eq!(client.dimensions(), 1_024);
    assert_eq!(client.max_inputs_per_batch(), 8);
    assert!(!client.contract_sha256().is_empty());
    let _ = format!("{client:?}");

    let batch = client.embeddings(&[String::from("hello")]).await.unwrap();
    assert_eq!(batch.embeddings.len(), 1);
    assert_eq!(batch.embeddings[0].len(), 1_024);
    assert_eq!(batch.prompt_tokens, Some(3));

    assert_eq!(
        client.embeddings(&[]).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
    assert_eq!(
        client.embeddings(&[String::new()]).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
    assert_eq!(
        client
            .embeddings(&(0..16).map(|i| format!("x{i}")).collect::<Vec<_>>())
            .await
            .unwrap_err(),
        AiProviderError::InvalidRequest
    );
}

#[tokio::test]
async fn exact_prefixed_endpoint_headers_and_compatible_response_are_supported() {
    let response = serde_json::to_vec(&serde_json::json!({
        "object": "list",
        "model": "provider-canonicalized-model",
        "provider_extension": true,
        "data": [
            {
                "object": "embedding",
                "index": 1,
                "embedding": vec![0.25_f32; 1_024],
                "extension": "ignored"
            },
            {
                "object": "embedding",
                "index": 0,
                "embedding": vec![0.5_f32; 1_024]
            }
        ],
        "usage": {"prompt_tokens": 4, "total_tokens": 4, "extension": 1}
    }))
    .unwrap();
    let (port, captured) = capture_one(response).await;
    let root = tempfile::tempdir().unwrap();
    let secret_path = root.path().join("api-key");
    fs::write(&secret_path, b"secret-value\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600)).unwrap();

    let mut config = embedding_config(&format!(
        "http://127.0.0.1:{port}/tenant/acme/compatible-mode/v1"
    ));
    let backend = config.backends.get_mut("fixture-embeddings").unwrap();
    backend.auth = AiAuthConfig::Header {
        name: "X-API-Key".to_owned(),
        secret: SecretReference {
            env: None,
            file: Some(secret_path),
        },
    };
    backend
        .headers
        .insert("X-Title".to_owned(), "open-compute".to_owned());
    config
        .embedding_profiles
        .get_mut("fixture/qwen3-1024")
        .unwrap()
        .send_dimensions = true;
    let contract = config.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&config, &contract).unwrap();
    let batch = client
        .embeddings(&["first".to_owned(), "second".to_owned()])
        .await
        .unwrap();
    assert_eq!(batch.embeddings[0][0], 0.5);
    assert_eq!(batch.embeddings[1][0], 0.25);

    let captured = captured.await.unwrap();
    assert_eq!(captured.path, "/tenant/acme/compatible-mode/v1/embeddings");
    assert_eq!(captured.headers["x-api-key"], "secret-value");
    assert_eq!(captured.headers["x-title"], "open-compute");
    let request: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
    assert_eq!(request["dimensions"], 1_024);
}

#[tokio::test]
async fn embeddings_maps_http_status_and_malformed_bodies() {
    let model = "@cf/qwen/qwen3-embedding-0.6b";
    let cases = vec![
        (
            StatusCode::UNAUTHORIZED,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Unauthorized,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::RateLimited {
                retry_after_seconds: Some(7),
            },
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Transient,
        ),
        (
            StatusCode::BAD_REQUEST,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Permanent,
        ),
        (
            StatusCode::OK,
            "text/plain",
            b"not-json".to_vec(),
            AiProviderError::MalformedResponse,
        ),
        (
            StatusCode::OK,
            "application/json",
            b"{nope".to_vec(),
            AiProviderError::MalformedResponse,
        ),
        (
            StatusCode::OK,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "object": "list",
                "model": "wrong",
                "data": []
            }))
            .unwrap(),
            AiProviderError::MalformedResponse,
        ),
        (
            StatusCode::OK,
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "object": "list",
                "model": model,
                "data": [{
                    "object": "embedding",
                    "index": 0,
                    "embedding": [0.0_f32, f32::NAN]
                }],
                "usage": {"prompt_tokens": 2, "total_tokens": 1}
            }))
            .unwrap(),
            AiProviderError::MalformedResponse,
        ),
    ];
    // Serve in reverse so pop order matches cases order.
    let responses = cases
        .iter()
        .rev()
        .map(|(s, ct, b, _)| (*s, *ct, b.clone()))
        .collect::<Vec<_>>();
    let port = ScriptedServer::new(responses).serve().await;
    let config = embedding_config(&format!("http://127.0.0.1:{port}/v1"));
    let contract = config.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&config, &contract).unwrap();
    for (_, _, _, expected) in cases {
        assert_eq!(
            client.embeddings(&[String::from("q")]).await.unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn embeddings_times_out_and_connection_failures_are_transient() {
    let config = embedding_config("http://127.0.0.1:1/v1");
    let contract = config.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&config, &contract).unwrap();
    assert_eq!(
        client.embeddings(&[String::from("q")]).await.unwrap_err(),
        AiProviderError::Transient
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        drop(stream);
    });
    let mut slow = embedding_config(&format!("http://127.0.0.1:{stall_port}/v1"));
    slow.provider_timeout_ms = 20;
    slow.query_timeout_ms = 20;
    let contract = slow.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&slow, &contract).unwrap();
    assert_eq!(
        client.embeddings(&[String::from("q")]).await.unwrap_err(),
        AiProviderError::Timeout
    );
}

#[tokio::test]
async fn chat_rewrite_rerank_and_stream_cover_happy_and_error_paths() {
    let sse =
        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
    // ScriptedServer pops from the end; first request uses the last vec entry.
    let responses = vec![
        (
            StatusCode::OK,
            "application/json",
            chat_ok_body("fixture-chat", "[1]"),
        ), // bad rerank index for one candidate
        (
            StatusCode::OK,
            "application/json",
            chat_ok_body("fixture-chat", ""),
        ), // empty rewrite
        (StatusCode::OK, "text/plain", b"nope".to_vec()), // bad content-type
        (StatusCode::UNAUTHORIZED, "application/json", b"{}".to_vec()),
        (
            StatusCode::OK,
            "application/json",
            chat_ok_body("fixture-chat", "rewritten query"),
        ),
        (
            StatusCode::OK,
            "application/json",
            chat_ok_body("fixture-chat", "[1,0]"),
        ),
        (
            StatusCode::OK,
            "application/json",
            chat_ok_body("fixture-chat", "hello"),
        ),
        (StatusCode::OK, "text/event-stream", sse.to_vec()),
    ];
    let port = ScriptedServer::new(responses).serve().await;
    let config = chat_config(&format!("http://127.0.0.1:{port}/v1"));
    let client =
        OpenAiChatClient::new(&config, "fixture/chat", AiGenerationCapability::Chat).unwrap();
    let _ = format!("{client:?}");

    let mut stream = client
        .chat_stream(&[ChatMessage::user("hi")], 32)
        .await
        .unwrap();
    assert_eq!(stream.next_delta().await.unwrap().as_deref(), Some("hi"));
    assert!(stream.next_delta().await.unwrap().is_none());
    assert!(stream.is_done());

    let completion = client.chat(&[ChatMessage::user("hi")], 32).await.unwrap();
    assert_eq!(completion.content, "hello");
    assert_eq!(completion.finish_reason, "stop");

    let order = client.rerank("q", &["a".into(), "b".into()]).await.unwrap();
    assert_eq!(order, vec![1, 0]);

    assert_eq!(
        client.rewrite_query("original").await.unwrap(),
        "rewritten query"
    );

    assert_eq!(
        client.chat(&[ChatMessage::user("x")], 1).await.unwrap_err(),
        AiProviderError::Unauthorized
    );
    assert_eq!(
        client.chat(&[ChatMessage::user("x")], 1).await.unwrap_err(),
        AiProviderError::MalformedResponse
    );
    assert_eq!(
        client.rewrite_query("x").await.unwrap_err(),
        AiProviderError::MalformedResponse
    );
    assert_eq!(
        client.rerank("q", &["only".into()]).await.unwrap_err(),
        AiProviderError::MalformedResponse
    );
    assert_eq!(
        client.rerank("q", &[]).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
    assert_eq!(
        client.chat(&[], 1).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
    assert_eq!(
        client.chat(&[ChatMessage::user("x")], 0).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
}

#[test]
fn client_constructors_fail_closed_on_contract_drift() {
    let config = embedding_config("http://127.0.0.1:9/v1");
    let mut contract = config.resolve_embedding_model(None).unwrap();
    contract.dimensions = 8;
    assert_eq!(
        OpenAiProviderClient::new(&config, &contract).unwrap_err(),
        AiProviderError::ContractMismatch
    );
    assert_eq!(
        OpenAiChatClient::new(&config, "missing", AiGenerationCapability::Chat).unwrap_err(),
        AiProviderError::ContractMismatch
    );
    let mut chat = chat_config("http://127.0.0.1:9/v1");
    chat.generation_models
        .get_mut("fixture/chat")
        .unwrap()
        .capabilities
        .clear();
    assert_eq!(
        OpenAiChatClient::new(&chat, "fixture/chat", AiGenerationCapability::Chat).unwrap_err(),
        AiProviderError::ContractMismatch
    );
}

#[test]
fn bearer_auth_resolves_from_env_secret() {
    let mut config = embedding_config("http://127.0.0.1:9/v1");
    config.backends.get_mut("fixture-embeddings").unwrap().auth = AiAuthConfig::Bearer {
        secret: SecretReference {
            env: Some("OPEN_COMPUTE_AI_PROVIDER_TEST_TOKEN".to_owned()),
            file: None,
        },
    };
    // Missing env fails closed.
    let contract = config.resolve_embedding_model(None).unwrap();
    assert_eq!(
        OpenAiProviderClient::new(&config, &contract).unwrap_err(),
        AiProviderError::ContractMismatch
    );
}

#[tokio::test]
async fn classify_status_covers_forbidden_redirect_and_rate_limit() {
    let responses = vec![
        (StatusCode::FOUND, "application/json", b"{}".to_vec()),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            b"{}".to_vec(),
        ),
        (StatusCode::FORBIDDEN, "application/json", b"{}".to_vec()),
    ];
    let port = ScriptedServer::new(responses).serve().await;
    let config = embedding_config(&format!("http://127.0.0.1:{port}/v1"));
    let contract = config.resolve_embedding_model(None).unwrap();
    let client = OpenAiProviderClient::new(&config, &contract).unwrap();
    assert_eq!(
        client.embeddings(&[String::from("q")]).await.unwrap_err(),
        AiProviderError::Unauthorized
    );
    assert_eq!(
        client.embeddings(&[String::from("q")]).await.unwrap_err(),
        AiProviderError::RateLimited {
            retry_after_seconds: Some(7)
        }
    );
    assert_eq!(
        client.embeddings(&[String::from("q")]).await.unwrap_err(),
        AiProviderError::Permanent
    );
}

#[tokio::test]
async fn chat_stream_rejects_malformed_choice_shapes_and_sends_bearer() {
    let sse_bad_choices =
        b"data: {\"choices\":[{\"index\":0,\"delta\":{}},{\"index\":1,\"delta\":{}}]}\n\n";
    let sse_ok =
        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"z\"}}]}\n\ndata: [DONE]\n\n";
    let responses = vec![
        (StatusCode::OK, "text/event-stream", sse_ok.to_vec()),
        (
            StatusCode::OK,
            "text/event-stream",
            sse_bad_choices.to_vec(),
        ),
    ];
    let port = ScriptedServer::new(responses).serve().await;
    let mut config = chat_config(&format!("http://127.0.0.1:{port}/v1"));
    let token_path = std::env::temp_dir().join(format!("oc-ai-token-{}.txt", std::process::id()));
    fs::write(&token_path, b"test-token\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();
    config.backends.get_mut("fixture-chat").unwrap().auth = AiAuthConfig::Bearer {
        secret: SecretReference {
            env: None,
            file: Some(token_path.clone()),
        },
    };
    let client =
        OpenAiChatClient::new(&config, "fixture/chat", AiGenerationCapability::Chat).unwrap();
    assert_eq!(
        client
            .chat_stream(&[ChatMessage::user("x")], 8)
            .await
            .unwrap()
            .next_delta()
            .await
            .unwrap_err(),
        AiProviderError::MalformedResponse
    );
    let mut stream = client
        .chat_stream(&[ChatMessage::user("x")], 8)
        .await
        .unwrap();
    assert_eq!(stream.next_delta().await.unwrap().as_deref(), Some("z"));
    assert!(stream.next_delta().await.unwrap().is_none());
    let _ = fs::remove_file(token_path);
}
