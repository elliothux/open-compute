use super::{WranglerCommand, assert_success, json_stdout};
use axum::Router;
use axum::body::Bytes;
use axum::routing::post;
use open_compute_artifacts::{AiSearchObjectStore, S3ArtifactClient};
use open_compute_core::{
    AiAuthConfig, AiConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiProviderConfig,
    AiTokenizer, AiTokenizerArtifactConfig, DocumentParserConfig,
};
use open_compute_service::document_parser_backend::DocumentParserBindingService;
use open_compute_service::{SearchApiState, VectorizeCoordinator};
use open_compute_storage::PlatformStorage;
use open_compute_workers::ResourcePins;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const VECTOR_INDEX: &str = "resource-gate-vectors";
const AI_NAMESPACE: &str = "resource-gate-search";
const AI_INSTANCE: &str = "resource-gate-ai";
const EMBEDDING_ALIAS: &str = "@cf/qwen/qwen3-embedding-0.6b";

pub(super) struct SearchFixture {
    pub(super) api: SearchApiState,
    coordinator: VectorizeCoordinator,
    embedding_task: tokio::task::JoinHandle<()>,
}

impl SearchFixture {
    pub(super) async fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        s3: S3ArtifactClient,
    ) -> Self {
        let (embedding_base_url, embedding_task) = spawn_embedding_fixture().await;
        let ai = ai_config(&embedding_base_url);
        let parser = Arc::new(DocumentParserBindingService::with_executable(
            storage.clone(),
            DocumentParserConfig::default(),
            PathBuf::from("/usr/bin/false"),
        ));
        let api = SearchApiState::new(
            storage.clone(),
            pins.clone(),
            storage.sqlite_busy_timeout_ms(),
            Duration::from_secs(1),
        )
        .with_ai_search_for_test(ai, AiSearchObjectStore::new(s3), parser)
        .unwrap();
        Self {
            api,
            coordinator: VectorizeCoordinator::new(storage, pins),
            embedding_task,
        }
    }

    fn drain_vectorize(&self) {
        assert_eq!(self.coordinator.drain_once().unwrap().applied, 1);
    }
}

impl Drop for SearchFixture {
    fn drop(&mut self) {
        self.embedding_task.abort();
    }
}

pub(super) async fn exercise_vectorize(
    command: &WranglerCommand<'_>,
    project: &Path,
    fixture: &SearchFixture,
) {
    assert_success(
        &command
            .run(&[
                "vectorize",
                "create",
                VECTOR_INDEX,
                "--dimensions",
                "3",
                "--metric",
                "cosine",
                "--description",
                "fixed Wrangler resource Gate",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    for args in [
        vec!["vectorize", "list", "--json", "--config", "wrangler.jsonc"],
        vec![
            "vectorize",
            "get",
            VECTOR_INDEX,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }

    std::fs::write(
        project.join("vectors-insert.ndjson"),
        concat!(
            "{\"id\":\"first\",\"values\":[1,0,0],\"metadata\":{\"kind\":\"primary\"}}\n",
            "{\"id\":\"second\",\"values\":[0,1,0],\"metadata\":{\"kind\":\"secondary\"}}\n"
        ),
    )
    .unwrap();
    assert_success(
        &command
            .run(&[
                "vectorize",
                "insert",
                VECTOR_INDEX,
                "--file",
                "vectors-insert.ndjson",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    fixture.drain_vectorize();

    std::fs::write(
        project.join("vectors-upsert.ndjson"),
        "{\"id\":\"first\",\"values\":[0,0,1],\"metadata\":{\"kind\":\"updated\"}}\n",
    )
    .unwrap();
    assert_success(
        &command
            .run(&[
                "vectorize",
                "upsert",
                VECTOR_INDEX,
                "--file",
                "vectors-upsert.ndjson",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    fixture.drain_vectorize();

    let fetched = command
        .run(&[
            "vectorize",
            "get-vectors",
            VECTOR_INDEX,
            "--ids",
            "first",
            "second",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&fetched);
    let fetched = String::from_utf8_lossy(&fetched.stdout);
    assert!(fetched.contains("first") && fetched.contains("second"));
    assert_success(
        &command
            .run(&[
                "vectorize",
                "query",
                VECTOR_INDEX,
                "--vector",
                "0",
                "0",
                "1",
                "--top-k",
                "2",
                "--return-values",
                "--return-metadata",
                "all",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "vectorize",
                "info",
                VECTOR_INDEX,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );

    for args in [
        vec![
            "vectorize",
            "create-metadata-index",
            VECTOR_INDEX,
            "--propertyName",
            "kind",
            "--type",
            "string",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "vectorize",
            "list-metadata-index",
            VECTOR_INDEX,
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "vectorize",
            "delete-metadata-index",
            VECTOR_INDEX,
            "--propertyName",
            "kind",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    assert_success(
        &command
            .run(&[
                "vectorize",
                "delete-vectors",
                VECTOR_INDEX,
                "--ids",
                "second",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    fixture.drain_vectorize();
    assert_success(
        &command
            .run(&[
                "vectorize",
                "delete",
                VECTOR_INDEX,
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

pub(super) async fn exercise_ai_search(command: &WranglerCommand<'_>) {
    assert_success(
        &command
            .run(&[
                "ai-search",
                "namespace",
                "create",
                AI_NAMESPACE,
                "--description",
                "fixed Wrangler resource Gate",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    for args in [
        vec![
            "ai-search",
            "namespace",
            "list",
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "namespace",
            "get",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "namespace",
            "update",
            AI_NAMESPACE,
            "--description",
            "updated by fixed Wrangler",
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    assert_success(
        &command
            .run(&[
                "ai-search",
                "create",
                AI_INSTANCE,
                "--namespace",
                AI_NAMESPACE,
                "--type",
                "builtin",
                "--embedding-model",
                EMBEDDING_ALIAS,
                "--chunk-size",
                "64",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    for args in [
        vec![
            "ai-search",
            "list",
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "get",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "update",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--max-num-results",
            "7",
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "stats",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "search",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--query",
            "local resource Gate",
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        let output = command.run(&args).await;
        assert!(
            output.status.success(),
            "fixed Wrangler command failed: {args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert_success(&output);
    }

    let created = command
        .run(&[
            "ai-search",
            "jobs",
            "create",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--description",
            "fixed Wrangler job",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&created);
    let job_id = json_stdout(&created)["id"].as_str().unwrap().to_owned();
    for args in [
        vec![
            "ai-search",
            "jobs",
            "list",
            AI_INSTANCE,
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "jobs",
            "get",
            AI_INSTANCE,
            &job_id,
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "ai-search",
            "jobs",
            "logs",
            AI_INSTANCE,
            &job_id,
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    assert_success(
        &command
            .run(&[
                "ai-search",
                "jobs",
                "cancel",
                AI_INSTANCE,
                &job_id,
                "--namespace",
                AI_NAMESPACE,
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "ai-search",
                "delete",
                AI_INSTANCE,
                "--namespace",
                AI_NAMESPACE,
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "ai-search",
                "namespace",
                "delete",
                AI_NAMESPACE,
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

fn ai_config(embedding_base_url: &str) -> AiConfig {
    let tokenizer_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer-word-level.json");
    let tokenizer = std::fs::read(&tokenizer_path).unwrap();
    let mut config = AiConfig::default();
    config.providers.insert(
        "resource-gate".to_owned(),
        AiProviderConfig {
            base_url: embedding_base_url.to_owned(),
            auth: AiAuthConfig::None,
        },
    );
    config.embedding_models.insert(
        EMBEDDING_ALIAS.to_owned(),
        AiEmbeddingModelConfig {
            provider: "resource-gate".to_owned(),
            remote_model: EMBEDDING_ALIAS.to_owned(),
            model_revision: "fixed-wrangler-resource-gate".to_owned(),
            dimensions: 1_024,
            request_dimensions: None,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 8_192,
            tokenizer: AiTokenizer::Qwen3,
            tokenizer_revision: "fixed-wrangler-resource-gate".to_owned(),
            tokenizer_artifact: AiTokenizerArtifactConfig {
                path: tokenizer_path,
                sha256: hex::encode(Sha256::digest(tokenizer)),
            },
        },
    );
    config.default_embedding_model = Some(EMBEDDING_ALIAS.to_owned());
    config
}

async fn spawn_embedding_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/embeddings", post(embedding_fixture)),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}/v1"), task)
}

async fn embedding_fixture(body: Bytes) -> axum::http::Response<String> {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return response(axum::http::StatusCode::BAD_REQUEST, String::new()),
    };
    if request.get("model") != Some(&Value::String(EMBEDDING_ALIAS.to_owned())) {
        return response(axum::http::StatusCode::BAD_REQUEST, String::new());
    }
    let inputs = match request.get("input") {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => match values
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
        {
            Some(value) => value,
            None => return response(axum::http::StatusCode::BAD_REQUEST, String::new()),
        },
        _ => return response(axum::http::StatusCode::BAD_REQUEST, String::new()),
    };
    let data = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            serde_json::json!({
                "object": "embedding",
                "index": index,
                "embedding": fixture_embedding(input),
            })
        })
        .collect::<Vec<_>>();
    response(
        axum::http::StatusCode::OK,
        serde_json::json!({
            "object": "list",
            "model": EMBEDDING_ALIAS,
            "data": data,
            "usage": {"prompt_tokens": inputs.len(), "total_tokens": inputs.len()},
        })
        .to_string(),
    )
}

fn fixture_embedding(text: &str) -> Vec<f32> {
    let mut values = vec![0.0_f32; 1_024];
    for token in text.split_whitespace() {
        let digest = Sha256::digest(token.as_bytes());
        let index = u32::from_le_bytes(digest[..4].try_into().unwrap()) as usize % values.len();
        values[index] += 1.0;
    }
    if values.iter().all(|value| *value == 0.0) {
        values[0] = 1.0;
    }
    values
}

fn response(status: axum::http::StatusCode, body: String) -> axum::http::Response<String> {
    axum::http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}
