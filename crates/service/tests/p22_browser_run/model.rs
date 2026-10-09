//! Loopback provider fixture for default and request-scoped Browser Run models.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};

pub(super) async fn spawn() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/chat", post(chat));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/chat"), task)
}

async fn chat(headers: HeaderMap, Json(request): Json<Value>) -> Response {
    let heading = match request["model"].as_str().unwrap() {
        "fixture-browser-model" => {
            assert!(!headers.contains_key("authorization"));
            "rendered"
        }
        "fixture-custom-model" => {
            if headers.get("authorization").and_then(|v| v.to_str().ok())
                != Some("Bearer custom-request-key")
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            "custom"
        }
        "fixture-fallback-model" => {
            assert_eq!(headers["authorization"], "Bearer fallback-request-key");
            "fallback"
        }
        _ => panic!("unexpected model"),
    };
    assert_eq!(request["stream"], false);
    let input: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert!(input["document"].as_str().unwrap().contains("# rendered"));
    if input["instructions"] == "rate-limit-fixture" {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let content = if input["instructions"] == "malformed-model" {
        "not JSON".to_owned()
    } else {
        format!("{{\"heading\":\"{heading}\"}}")
    };
    Json(json!({"model":request["model"],"choices":[{"index":0,"message":{"role":"assistant","content":content},"finish_reason":"stop"}]})).into_response()
}

pub(super) fn configuration(endpoint: &str) -> String {
    format!(
        r#"
[ai]
default_generation_model = "fixture/browser-json"
[ai.backends.browser-json]
protocol = "openai_chat_completions_v1"
endpoint = "{endpoint}"
auth = {{ kind = "none" }}
[ai.generation_models."fixture/browser-json"]
backend = "browser-json"
remote_model = "fixture-browser-model"
max_context_tokens = 8192
capabilities = ["chat"]
[ai.generation_models."openai/gpt-4o"]
backend = "browser-json"
remote_model = "fixture-custom-model"
max_context_tokens = 8192
capabilities = ["chat"]
[ai.generation_models."anthropic/claude-sonnet-4-20250514"]
backend = "browser-json"
remote_model = "fixture-fallback-model"
max_context_tokens = 8192
capabilities = ["chat"]
[ai.generation_models."workers-ai/@cf/meta/llama-3.3-70b-instruct-fp8-fast"]
backend = "browser-json"
remote_model = "fixture-browser-model"
max_context_tokens = 8192
capabilities = ["chat"]
"#
    )
}
