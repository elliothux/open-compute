use super::*;
use bytes::Bytes;
use http_body_util::BodyExt as _;
use http_body_util::Full;
use hyper::body::Incoming as HyperIncoming;
use hyper::header::{CONTENT_TYPE, RETRY_AFTER};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request as HyperRequest, Response, StatusCode};
use hyper_util::rt::TokioIo;
use open_compute_core::{
    AiAuthConfig, AiBackendConfig, AiBackendProtocol, AiConfig, AiEmbeddingModelConfig,
    AiEmbeddingProfileConfig, AiGenerationCapability, AiGenerationModelConfig,
    AiRerankingModelConfig, AiTokenizer, AiTokenizerArtifactConfig, AiTokenizerConfig,
    AiVlmModelConfig, OperatorProxyPolicy, SecretReference,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

type ScriptedResponse = (StatusCode, &'static str, Vec<u8>);

#[tokio::test]
async fn embedding_request_uses_the_selected_operator_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let count = stream.read(&mut request).await.unwrap();
        let request = std::str::from_utf8(&request[..count]).unwrap();
        assert!(request.starts_with("POST http://provider.invalid/v1 HTTP/1.1\r\n"));
        let body =
            r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[1.0]}]}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let policy =
        OperatorProxyPolicy::from_lookup(|name| (name == "HTTPS_PROXY").then(|| proxy.clone()))
            .unwrap();
    let client = OpenAiProviderClient {
        transport: crate::operator_http::OperatorHttpClient::new(policy).unwrap(),
        endpoint: "http://provider.invalid/v1".parse().unwrap(),
        remote_model: "fixture".to_owned(),
        contract_sha256: hex::encode([1; 32]),
        dimensions: 1,
        request_dimensions: None,
        headers: HeaderMap::new(),
        max_inputs: 1,
        max_request_bytes: 4096,
        max_response_bytes: 4096,
        timeout: Duration::from_secs(1),
    };
    assert_eq!(
        client
            .embeddings(&["hello".to_owned()])
            .await
            .unwrap()
            .embeddings,
        vec![vec![1.0]]
    );
    server.await.unwrap();
}

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

fn rerank_config(root: &str, protocol: AiBackendProtocol) -> AiConfig {
    let mut config = chat_config(root);
    config.backends.insert(
        "fixture-rerank".to_owned(),
        AiBackendConfig {
            protocol,
            endpoint: format!("{root}/rerank"),
            auth: AiAuthConfig::None,
            headers: Default::default(),
        },
    );
    config.reranking_models.insert(
        "fixture/rerank".to_owned(),
        AiRerankingModelConfig {
            backend: "fixture-rerank".to_owned(),
            remote_model: "fixture-reranker".to_owned(),
            provider_revision: Some("rev-1".to_owned()),
        },
    );
    config.default_reranking_model = Some("fixture/rerank".to_owned());
    config
}

fn vision_config(root: &str) -> AiConfig {
    let mut config = chat_config(root);
    config.vlm_models.insert(
        "fixture/vision".to_owned(),
        AiVlmModelConfig {
            backend: "fixture-chat".to_owned(),
            remote_model: "fixture-vision".to_owned(),
            provider_revision: None,
            max_input_width: 1_280,
            max_input_height: 720,
            max_input_pixels: 921_600,
            max_encoded_image_bytes: 4 * 1024 * 1024,
            max_output_tokens: 1_024,
        },
    );
    config.default_vlm_model = Some("fixture/vision".to_owned());
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

#[tokio::test]
async fn custom_ai_chat_request_credentials_replace_operator_auth_without_resolving_or_mutating_it()
{
    let root = tempfile::tempdir().unwrap();
    let missing = SecretReference {
        env: None,
        file: Some(root.path().join("absent-operator-key")),
    };
    for (auth, header, expected) in [
        (
            AiAuthConfig::Bearer {
                secret: missing.clone(),
            },
            "authorization",
            "Bearer request-key",
        ),
        (
            AiAuthConfig::Header {
                name: "x-api-key".into(),
                secret: missing.clone(),
            },
            "x-api-key",
            "request-key",
        ),
        (AiAuthConfig::None, "authorization", "Bearer request-key"),
    ] {
        let (port, captured) = capture_one(chat_ok_body("fixture-chat", "done")).await;
        let mut config = chat_config(&format!("http://127.0.0.1:{port}/v1"));
        let backend = config.backends.get_mut("fixture-chat").unwrap();
        backend.auth = auth.clone();
        backend
            .headers
            .insert("x-provider-metadata".into(), "configured".into());
        let original = config.clone();
        if auth != AiAuthConfig::None {
            assert_eq!(
                OpenAiChatClient::new(&config, "fixture/chat", AiGenerationCapability::Chat)
                    .unwrap_err(),
                AiProviderError::ContractMismatch
            );
        }
        for invalid in ["", "Bearer ", "key\r\nInjected: value", "a b", "é"] {
            assert_eq!(
                OpenAiChatClient::with_request_authorization(&config, "fixture/chat", invalid)
                    .unwrap_err(),
                AiProviderError::InvalidRequest
            );
        }
        assert_eq!(
            OpenAiChatClient::with_request_authorization(&config, "missing/model", "key")
                .unwrap_err(),
            AiProviderError::ContractMismatch
        );
        let client = OpenAiChatClient::with_request_authorization(
            &config,
            "fixture/chat",
            "Bearer request-key",
        )
        .unwrap();
        assert!(client.headers[header].is_sensitive());
        assert!(!format!("{client:?}").contains("request-key"));
        assert!(!format!("{:?}", client.headers).contains("request-key"));
        assert_eq!(
            client
                .chat(&[ChatMessage::user("extract")], 16)
                .await
                .unwrap()
                .content,
            "done"
        );
        let captured = captured.await.unwrap();
        assert_eq!(captured.headers[header], expected);
        if header != "authorization" {
            assert!(!captured.headers.contains_key("authorization"));
        }
        assert_eq!(captured.headers["x-provider-metadata"], "configured");
        let body: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
        assert_eq!(body["model"], "fixture-chat");
        assert_eq!(config, original);
    }
}

#[tokio::test]
async fn vision_request_is_fixed_bounded_and_multimodal() {
    let (port, captured) = capture_one(chat_ok_body("fixture-vision", "A useful diagram.")).await;
    let config = vision_config(&format!("http://127.0.0.1:{port}/v1"));
    let contract = config
        .resolve_default_vlm_model()
        .unwrap()
        .expect("configured VLM");
    let client = OpenAiVisionClient::new(&config, &contract).unwrap();
    let description = client.describe("AQID", "fr").await.unwrap();
    assert_eq!(description, "A useful diagram.");
    let captured = captured.await.unwrap();
    assert_eq!(captured.path, "/v1/chat/completions");
    let request: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
    assert_eq!(request["model"], "fixture-vision");
    assert_eq!(request["temperature"], 0);
    assert_eq!(request["stream"], false);
    assert_eq!(request["messages"][1]["content"][1]["type"], "image_url");
    assert_eq!(
        request["messages"][1]["content"][1]["image_url"]["url"],
        "data:image/jpeg;base64,AQID"
    );
    assert!(
        request["messages"][1]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("French")
    );
    assert_eq!(
        client.describe("AQID", "zh").await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
}

#[tokio::test]
async fn vision_provider_status_and_response_failures_are_sanitized() {
    for (status, content_type, body, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            "application/json",
            b"secret provider body".to_vec(),
            AiProviderError::Unauthorized,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "application/json",
            b"limited".to_vec(),
            AiProviderError::RateLimited {
                retry_after_seconds: Some(7),
            },
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            b"failed".to_vec(),
            AiProviderError::Transient,
        ),
        (
            StatusCode::TEMPORARY_REDIRECT,
            "text/plain",
            b"redirect".to_vec(),
            AiProviderError::Permanent,
        ),
        (
            StatusCode::OK,
            "text/plain",
            b"not json".to_vec(),
            AiProviderError::MalformedResponse,
        ),
        (
            StatusCode::OK,
            "application/json",
            br#"{"model":"fixture-vision","choices":[]}"#.to_vec(),
            AiProviderError::MalformedResponse,
        ),
    ] {
        let port = ScriptedServer::new(vec![(status, content_type, body)])
            .serve()
            .await;
        let config = vision_config(&format!("http://127.0.0.1:{port}/v1"));
        let contract = config.resolve_default_vlm_model().unwrap().unwrap();
        let client = OpenAiVisionClient::new(&config, &contract).unwrap();
        assert_eq!(client.describe("AQID", "en").await.unwrap_err(), expected);
    }

    let port = ScriptedServer::new(vec![(
        StatusCode::OK,
        "application/json",
        chat_ok_body("fixture-vision", "description exceeding the configured cap"),
    )])
    .serve()
    .await;
    let mut config = vision_config(&format!("http://127.0.0.1:{port}/v1"));
    config.max_vlm_response_bytes = 16;
    let contract = config.resolve_default_vlm_model().unwrap().unwrap();
    let client = OpenAiVisionClient::new(&config, &contract).unwrap();
    assert_eq!(
        client.describe("AQID", "en").await.unwrap_err(),
        AiProviderError::MalformedResponse
    );
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
async fn chat_rewrite_and_stream_cover_happy_and_error_paths() {
    let sse =
        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
    // ScriptedServer pops from the end; first request uses the last vec entry.
    let responses = vec![
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
        client.chat(&[], 1).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
    assert_eq!(
        client.chat(&[ChatMessage::user("x")], 0).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
}

#[tokio::test]
async fn dedicated_rerank_adapters_send_exact_wire_shapes_and_normalize_scores() {
    for (protocol, expected) in [
        (
            AiBackendProtocol::CohereRerankV2,
            serde_json::json!({
                "model": "fixture-reranker",
                "query": "query",
                "documents": ["first", "second"],
                "top_n": 2,
            }),
        ),
        (
            AiBackendProtocol::RerankV1,
            serde_json::json!({
                "model": "fixture-reranker",
                "query": "query",
                "documents": ["first", "second"],
            }),
        ),
    ] {
        let response = serde_json::to_vec(&serde_json::json!({
            "results": [
                {"index": 1, "relevance_score": 0.75, "ignored": true},
                {"index": 0, "relevance_score": 0.75}
            ]
        }))
        .unwrap();
        let (port, captured) = capture_one(response).await;
        let config = rerank_config(&format!("http://127.0.0.1:{port}"), protocol);
        let client = RerankClient::new(&config, "fixture/rerank").unwrap();
        let results = client
            .rerank("query", &["first".into(), "second".into()])
            .await
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|result| result.index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        let request = captured.await.unwrap();
        assert_eq!(request.path, "/rerank");
        assert_eq!(request.headers[CONTENT_TYPE], "application/json");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn dedicated_rerank_rejects_partial_duplicate_out_of_range_and_invalid_scores() {
    let bodies = [
        serde_json::json!({"results": [{"index": 0, "relevance_score": 0.5}]}),
        serde_json::json!({"results": [
            {"index": 0, "relevance_score": 0.5},
            {"index": 0, "relevance_score": 0.4}
        ]}),
        serde_json::json!({"results": [
            {"index": 0, "relevance_score": 0.5},
            {"index": 2, "relevance_score": 0.4}
        ]}),
        serde_json::json!({"results": [
            {"index": 0, "relevance_score": -0.1},
            {"index": 1, "relevance_score": 0.4}
        ]}),
        serde_json::json!({"results": [
            {"index": 0, "relevance_score": 0.5},
            {"index": 1, "relevance_score": 1.1}
        ]}),
    ];
    let responses = bodies
        .into_iter()
        .rev()
        .map(|body| {
            (
                StatusCode::OK,
                "application/json",
                serde_json::to_vec(&body).unwrap(),
            )
        })
        .collect();
    let port = ScriptedServer::new(responses).serve().await;
    let config = rerank_config(
        &format!("http://127.0.0.1:{port}"),
        AiBackendProtocol::RerankV1,
    );
    let client = RerankClient::new(&config, "fixture/rerank").unwrap();
    for _ in 0..5 {
        assert_eq!(
            client
                .rerank("query", &["first".into(), "second".into()])
                .await
                .unwrap_err(),
            AiProviderError::MalformedResponse
        );
    }
    assert_eq!(
        client.rerank("query", &[]).await.unwrap_err(),
        AiProviderError::InvalidRequest
    );
}

#[tokio::test]
async fn dedicated_rerank_maps_transport_status_and_body_failures() {
    let valid = serde_json::to_vec(&serde_json::json!({
        "results": [{"index": 0, "relevance_score": 0.5}]
    }))
    .unwrap();
    let cases = [
        (
            StatusCode::UNAUTHORIZED,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Unauthorized,
        ),
        (
            StatusCode::FORBIDDEN,
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
            StatusCode::BAD_REQUEST,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Permanent,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Transient,
        ),
        (
            StatusCode::FOUND,
            "application/json",
            b"{}".to_vec(),
            AiProviderError::Permanent,
        ),
        (
            StatusCode::OK,
            "text/plain",
            valid.clone(),
            AiProviderError::MalformedResponse,
        ),
        (
            StatusCode::OK,
            "application/json",
            b"{nope".to_vec(),
            AiProviderError::MalformedResponse,
        ),
    ];
    let responses = cases
        .iter()
        .rev()
        .map(|(status, content_type, body, _)| (*status, *content_type, body.clone()))
        .collect();
    let port = ScriptedServer::new(responses).serve().await;
    let config = rerank_config(
        &format!("http://127.0.0.1:{port}"),
        AiBackendProtocol::RerankV1,
    );
    let client = RerankClient::new(&config, "fixture/rerank").unwrap();
    for (_, _, _, expected) in cases {
        assert_eq!(
            client
                .rerank("query", &["document".into()])
                .await
                .unwrap_err(),
            expected
        );
    }

    let port = ScriptedServer::new(vec![(StatusCode::OK, "application/json", valid)])
        .serve()
        .await;
    let mut limited = rerank_config(
        &format!("http://127.0.0.1:{port}"),
        AiBackendProtocol::RerankV1,
    );
    limited.max_provider_response_bytes = 8;
    assert_eq!(
        RerankClient::new(&limited, "fixture/rerank")
            .unwrap()
            .rerank("query", &["document".into()])
            .await
            .unwrap_err(),
        AiProviderError::MalformedResponse
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(stream);
    });
    let mut slow = rerank_config(
        &format!("http://127.0.0.1:{port}"),
        AiBackendProtocol::RerankV1,
    );
    slow.provider_timeout_ms = 10;
    slow.query_timeout_ms = 10;
    assert_eq!(
        RerankClient::new(&slow, "fixture/rerank")
            .unwrap()
            .rerank("query", &["document".into()])
            .await
            .unwrap_err(),
        AiProviderError::Timeout
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
