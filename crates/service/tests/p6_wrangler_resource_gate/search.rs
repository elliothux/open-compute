use super::{WranglerCommand, assert_success, json_stdout};
use axum::Router;
use axum::body::Bytes;
use axum::routing::post;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const VECTOR_INDEX: &str = "resource-gate-vectors";
const LARGE_VECTOR_INDEX: &str = "resource-gate-vectors-large";
const AI_NAMESPACE: &str = "resource-gate-search";
const AI_INSTANCE: &str = "resource-gate-ai";
const EMBEDDING_ALIAS: &str = "@cf/qwen/qwen3-embedding-0.6b";

pub(super) struct EmbeddingFixture {
    pub(super) base_url: String,
    task: tokio::task::JoinHandle<()>,
}

impl EmbeddingFixture {
    pub(super) async fn spawn() -> Self {
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
        Self {
            base_url: format!("http://{address}/v1"),
            task,
        }
    }
}

impl Drop for EmbeddingFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn ai_config_toml(embedding_base_url: &str) -> String {
    let tokenizer_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tokenizer-word-level.json")
        .canonicalize()
        .expect("in-repo tokenizer fixture");
    let tokenizer = std::fs::read(&tokenizer_path).unwrap();
    format!(
        r#"[ai]
default_embedding_model = "{EMBEDDING_ALIAS}"

[ai.providers.resource-gate]
base_url = {embedding_base_url}
auth = {{ kind = "none" }}

[ai.embedding_models."{EMBEDDING_ALIAS}"]
provider = "resource-gate"
remote_model = "{EMBEDDING_ALIAS}"
model_revision = "fixed-wrangler-resource-gate"
dimensions = 1024
metric = "cosine"
max_input_tokens = 8192
tokenizer = "qwen3"
tokenizer_revision = "fixed-wrangler-resource-gate"
tokenizer_artifact = {{ path = {tokenizer_path}, sha256 = "{tokenizer_sha256}" }}
"#,
        embedding_base_url = toml::Value::String(embedding_base_url.to_owned()),
        tokenizer_path = toml::Value::String(tokenizer_path.display().to_string()),
        tokenizer_sha256 = hex::encode(Sha256::digest(tokenizer)),
    )
}

pub(super) async fn exercise_vectorize(command: &WranglerCommand<'_>, project: &Path) {
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
    wait_for_vector_text(command, VECTOR_INDEX, "first", "\"kind\": \"primary\"").await;
    wait_for_vector_text(command, VECTOR_INDEX, "second", "\"kind\": \"secondary\"").await;

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
    wait_for_vector_text(command, VECTOR_INDEX, "first", "\"kind\": \"updated\"").await;

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
    let query = command
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
        .await;
    assert_success(&query);
    let query = String::from_utf8_lossy(&query.stdout);
    assert!(query.contains("\"id\": \"first\""));
    assert!(query.contains("\"kind\": \"updated\""));
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

    assert_success(
        &command
            .run(&[
                "vectorize",
                "create-metadata-index",
                VECTOR_INDEX,
                "--propertyName",
                "kind",
                "--type",
                "string",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let metadata = command
        .run(&[
            "vectorize",
            "list-metadata-index",
            VECTOR_INDEX,
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&metadata);
    assert!(json_stdout(&metadata).as_array().is_some_and(|indexes| {
        indexes
            .iter()
            .any(|index| index["propertyName"] == "kind" && index["indexType"] == "string")
    }));
    assert_success(
        &command
            .run(&[
                "vectorize",
                "delete-metadata-index",
                VECTOR_INDEX,
                "--propertyName",
                "kind",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
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
    wait_for_vector_absent(command, VECTOR_INDEX, "second").await;
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

    assert_success(
        &command
            .run(&[
                "vectorize",
                "create",
                LARGE_VECTOR_INDEX,
                "--dimensions",
                "1200",
                "--metric",
                "cosine",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let mut large_vectors = String::new();
    for id in 0..1_000 {
        large_vectors.push_str(
            &serde_json::json!({
                "id": format!("large-{id}"),
                "values": vec![0; 1_200],
            })
            .to_string(),
        );
        large_vectors.push('\n');
    }
    assert!(large_vectors.len() > 2 * 1024 * 1024);
    assert!(large_vectors.len() < 24 * 1024 * 1024);
    std::fs::write(project.join("vectors-large.ndjson"), large_vectors).unwrap();
    assert_success(
        &command
            .run(&[
                "vectorize",
                "insert",
                LARGE_VECTOR_INDEX,
                "--file",
                "vectors-large.ndjson",
                "--batch-size",
                "1000",
                "--json",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    wait_for_vector_text(
        command,
        LARGE_VECTOR_INDEX,
        "large-999",
        "\"id\": \"large-999\"",
    )
    .await;
    assert_success(
        &command
            .run(&[
                "vectorize",
                "delete",
                LARGE_VECTOR_INDEX,
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

pub(super) async fn exercise_ai_search(command: &WranglerCommand<'_>) {
    let created_namespace = command
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
        .await;
    assert_success(&created_namespace);
    let created_namespace = json_stdout(&created_namespace);
    assert_eq!(created_namespace["name"], AI_NAMESPACE);
    assert_eq!(
        created_namespace["description"],
        "fixed Wrangler resource Gate"
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
        let output = command.run(&args).await;
        assert_success(&output);
        let value = json_stdout(&output);
        match args[2] {
            "list" => assert!(value.as_array().is_some_and(|namespaces| {
                namespaces
                    .iter()
                    .any(|namespace| namespace["name"] == AI_NAMESPACE)
            })),
            "get" => assert_eq!(value["description"], "fixed Wrangler resource Gate"),
            "update" => assert_eq!(value["description"], "updated by fixed Wrangler"),
            _ => unreachable!(),
        }
    }
    let created_instance = command
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
        .await;
    assert_success(&created_instance);
    let created_instance = json_stdout(&created_instance);
    assert_eq!(created_instance["id"], AI_INSTANCE);
    assert_eq!(created_instance["namespace"], AI_NAMESPACE);
    assert_eq!(created_instance["chunk_size"], 64);
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
        assert_success(&output);
        let value = json_stdout(&output);
        match args[1] {
            "list" => assert!(value.as_array().is_some_and(|instances| {
                instances
                    .iter()
                    .any(|instance| instance["id"] == AI_INSTANCE)
            })),
            "get" => assert_eq!(value["id"], AI_INSTANCE),
            "update" => assert_eq!(value["max_num_results"], 7),
            "stats" => {
                for field in [
                    "queued",
                    "running",
                    "completed",
                    "skipped",
                    "outdated",
                    "error",
                ] {
                    assert!(
                        value[field].is_number(),
                        "missing numeric AI Search stat {field}"
                    );
                }
            }
            "search" => {
                assert_eq!(value["search_query"], "local resource Gate");
                assert!(value["chunks"].is_array());
            }
            _ => unreachable!(),
        }
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
        let output = command.run(&args).await;
        assert_success(&output);
        let value = json_stdout(&output);
        match args[2] {
            "list" => assert!(
                value
                    .as_array()
                    .is_some_and(|jobs| { jobs.iter().any(|job| job["id"] == job_id) })
            ),
            "get" => {
                assert_eq!(value["id"], job_id);
                assert_eq!(value["description"], "fixed Wrangler job");
            }
            "logs" => assert!(value.is_array()),
            _ => unreachable!(),
        }
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
    let instances_after_delete = command
        .run(&[
            "ai-search",
            "list",
            "--namespace",
            AI_NAMESPACE,
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&instances_after_delete);
    assert!(
        json_stdout(&instances_after_delete)
            .as_array()
            .is_some_and(|instances| instances
                .iter()
                .all(|instance| instance["id"] != AI_INSTANCE))
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
    let namespaces_after_delete = command
        .run(&[
            "ai-search",
            "namespace",
            "list",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&namespaces_after_delete);
    assert!(
        json_stdout(&namespaces_after_delete)
            .as_array()
            .is_some_and(|namespaces| namespaces
                .iter()
                .all(|namespace| namespace["name"] != AI_NAMESPACE))
    );
}

async fn wait_for_vector_text(
    command: &WranglerCommand<'_>,
    index: &str,
    id: &str,
    expected: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let output = command
            .run(&[
                "vectorize",
                "get-vectors",
                index,
                "--ids",
                id,
                "--config",
                "wrangler.jsonc",
            ])
            .await;
        assert_success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Vectorize mutation: index={index} id={id} expected={expected} stdout={stdout} stderr={}",
            String::from_utf8_lossy(&output.stderr),
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_for_vector_absent(command: &WranglerCommand<'_>, index: &str, id: &str) {
    let needle = format!("\"id\": \"{id}\"");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let output = command
            .run(&[
                "vectorize",
                "get-vectors",
                index,
                "--ids",
                id,
                "--config",
                "wrangler.jsonc",
            ])
            .await;
        assert_success(&output);
        if !String::from_utf8_lossy(&output.stdout).contains(&needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for Vectorize deletion: index={index} id={id}",
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
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
