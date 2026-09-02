use super::*;
use open_compute_core::PlatformConfig;

fn catalog() -> AiConfig {
    PlatformConfig::from_toml_str(
        r#"
[ai]
default_embedding_model = "@cf/qwen/qwen3-embedding-0.6b"

[ai.providers.fixture]
base_url = "http://127.0.0.1:8080/v1"
auth = { kind = "none" }

[ai.embedding_models."@cf/qwen/qwen3-embedding-0.6b"]
provider = "fixture"
remote_model = "@cf/qwen/qwen3-embedding-0.6b"
model_revision = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3"
dimensions = 1024
metric = "cosine"
max_input_tokens = 8192
tokenizer = "qwen3"
tokenizer_revision = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3"
tokenizer_artifact = { path = "/opt/open-compute/models/qwen3/tokenizer.json", sha256 = "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a" }
"#,
    )
    .unwrap()
    .ai
}

#[test]
fn create_config_resolves_default_model_and_canonical_contract() {
    let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
        "id": "knowledge_base",
        "index_method": {"vector": true, "keyword": true},
        "fusion_method": "rrf",
        "indexing_options": {"keyword_tokenizer": "porter"},
        "retrieval_options": {"keyword_match_mode": "and"},
        "chunk_size": 512,
        "chunk_overlap": 10,
        "custom_metadata": [{"field_name": "language", "data_type": "text"}]
    }))
    .unwrap();
    let prepared = input.prepare(&catalog()).unwrap();
    assert_eq!(prepared.dimensions, 1024);
    assert!(prepared.vector_enabled);
    assert!(prepared.keyword_enabled);
    assert_eq!(
        Sha256::digest(&prepared.model_contract_json).as_slice(),
        prepared.model_contract_sha256
    );
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["embedding_model"], "@cf/qwen/qwen3-embedding-0.6b");
}

#[test]
fn keyword_only_and_fail_closed_options_are_explicit() {
    let keyword: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
        "id": "keyword-only",
        "index_method": {"vector": false, "keyword": true},
        "indexing_options": {"keyword_tokenizer": "trigram"}
    }))
    .unwrap();
    let prepared = keyword.prepare(&catalog()).unwrap();
    assert_eq!(prepared.dimensions, 0);
    assert!(prepared.embedding_contract.is_none());
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["embedding_model"], "@cf/qwen/qwen3-embedding-0.6b");
    let tokenizer = parse_keyword_only_tokenizer_contract(&prepared.model_contract_json).unwrap();
    assert_eq!(tokenizer.tokenizer, open_compute_core::AiTokenizer::Qwen3);
    assert_eq!(public["retrieval_options"]["keyword_match_mode"], "and");

    for value in [
        serde_json::json!({"id":"bad", "index_method":{"vector":false,"keyword":false}}),
        serde_json::json!({"id":"bad", "chunk_overlap":31}),
        serde_json::json!({"id":"bad", "index_method":{"vector":true,"keyword":false}, "fusion_method":"max"}),
    ] {
        let input: AiSearchCreateInput = serde_json::from_value(value).unwrap();
        assert!(input.prepare(&catalog()).is_err());
    }
    assert!(
        serde_json::from_value::<AiSearchCreateInput>(serde_json::json!({
            "id":"bad", "ai_gateway_id":"tenant-selects-provider"
        }))
        .is_err()
    );
}
