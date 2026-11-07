use super::*;
use open_compute_core::PlatformConfig;

fn catalog() -> AiConfig {
    PlatformConfig::from_toml_str(
        r#"
[data]
path = "/tmp/open-compute-ai-config-data"
master_key_file = "/tmp/open-compute-ai-config-data/keys/master.key"

[storage]
backend = "local"
prefix = "system/"

[ai]
default_embedding_model = "@cf/qwen/qwen3-embedding-0.6b"
default_reranking_model = "@cf/baai/bge-reranker-base"

[ai.backends.fixture]
protocol = "openai_embeddings_v1"
endpoint = "http://127.0.0.1:8080/v1/embeddings"
auth = { kind = "none" }

[ai.backends.rerank]
protocol = "cohere_rerank_v2"
endpoint = "http://127.0.0.1:8080/v2/rerank"
auth = { kind = "none" }

[ai.embedding_profiles."fixture/qwen3"]
dimensions = 1024
max_input_tokens = 8192
tokenizer = { kind = "qwen3", revision = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3", artifact = { path = "/opt/open-compute/models/qwen3/tokenizer.json", sha256 = "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a" } }

[ai.embedding_models."@cf/qwen/qwen3-embedding-0.6b"]
backend = "fixture"
remote_model = "@cf/qwen/qwen3-embedding-0.6b"
provider_revision = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3"
profile = "fixture/qwen3"

[ai.reranking_models."@cf/baai/bge-reranker-base"]
backend = "rerank"
remote_model = "rerank-v4.0-fast"
provider_revision = "2026-09-01"
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
fn disabled_rewrite_accepts_a_chat_only_default_but_validates_explicit_models() {
    use open_compute_core::{
        AiAuthConfig, AiBackendConfig, AiBackendProtocol, AiGenerationModelConfig,
    };

    let mut catalog = catalog();
    catalog.backends.insert(
        "chat".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiChatCompletionsV1,
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_owned(),
            auth: AiAuthConfig::None,
            headers: BTreeMap::new(),
        },
    );
    catalog.generation_models.insert(
        "fixture/chat-only".to_owned(),
        AiGenerationModelConfig {
            backend: "chat".to_owned(),
            remote_model: "fixture/chat-only".to_owned(),
            provider_revision: None,
            max_context_tokens: 4096,
            capabilities: BTreeSet::from([AiGenerationCapability::Chat]),
        },
    );
    catalog.default_generation_model = Some("fixture/chat-only".to_owned());
    let input: AiSearchCreateInput =
        serde_json::from_value(serde_json::json!({"id":"ordinary"})).unwrap();
    let prepared = input.prepare(&catalog).unwrap();
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["ai_search_model"], "fixture/chat-only");
    assert_eq!(public["rewrite_query"], false);
    for fields in [
        serde_json::json!({"id":"rewrite", "rewrite_query":true}),
        serde_json::json!({"id":"explicit", "rewrite_model":"fixture/chat-only"}),
        serde_json::json!({"id":"missing", "rewrite_model":"missing"}),
    ] {
        let input: AiSearchCreateInput = serde_json::from_value(fields).unwrap();
        assert_eq!(
            input.prepare(&catalog).unwrap_err().code(),
            ErrorCode::BindingCapabilityUnsupported
        );
    }
    catalog
        .generation_models
        .get_mut("fixture/chat-only")
        .unwrap()
        .capabilities
        .insert(AiGenerationCapability::Rewrite);
    let input: AiSearchCreateInput =
        serde_json::from_value(serde_json::json!({"id":"enabled", "rewrite_query":true})).unwrap();
    let prepared = input.prepare(&catalog).unwrap();
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["rewrite_query"], true);
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

#[test]
fn chunk_false_round_trips_and_rejects_chunk_parameters() {
    let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
        "id": "whole-document",
        "chunk": false,
        "index_method": {"vector": false, "keyword": true}
    }))
    .unwrap();
    let prepared = input.prepare(&catalog()).unwrap();
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["chunk"], false);

    for field in ["chunk_size", "chunk_overlap"] {
        let mut value = serde_json::json!({"id": "invalid", "chunk": false});
        value[field] = serde_json::json!(1);
        let input: AiSearchCreateInput = serde_json::from_value(value).unwrap();
        assert_eq!(
            input.prepare(&catalog()).unwrap_err().code(),
            ErrorCode::BindingCapabilityUnsupported
        );
    }
}

#[test]
fn r2_source_config_is_strict_canonical_and_accepts_current_intervals() {
    let token = stable_ai_search_token_id("account");
    for interval in [900, 1_800, 3_600, 7_200, 14_400, 21_600, 43_200, 86_400] {
        let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
            "id": "a".repeat(64),
            "type": "r2",
            "source": "documents",
            "source_params": {
                "prefix": "docs/",
                "include_items": ["**/*.pdf"],
                "exclude_items": ["**/*.tmp"]
            },
            "token_id": token,
            "sync_interval": interval
        }))
        .unwrap();
        let prepared = input.prepare(&catalog()).unwrap();
        let config: ResolvedAiSearchConfig =
            serde_json::from_slice(&prepared.public_config_json).unwrap();
        assert_eq!(config.source_type.as_deref(), Some("r2"));
        assert_eq!(config.sync_interval, Some(interval));
    }

    for value in [
        serde_json::json!({"id":"bad", "source":"documents"}),
        serde_json::json!({"id":"bad", "type":"web-crawler", "source":"site"}),
        serde_json::json!({"id":"bad", "type":"r2", "source":"documents", "token_id":token, "sync_interval":60}),
        serde_json::json!({"id":"bad", "type":"r2", "source":"documents", "token_id":token, "source_params":{"include_items":["[bad]"]}}),
    ] {
        let input: AiSearchCreateInput = serde_json::from_value(value).unwrap();
        assert!(input.prepare(&catalog()).is_err());
    }
}

#[test]
fn reranking_and_boosting_resolve_independently_and_fail_closed() {
    let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
        "id": "ranked",
        "reranking": true,
        "index_method": {"vector": true, "keyword": true},
        "custom_metadata": [
            {"field_name": "Priority", "data_type": "number"},
            {"field_name": "draft", "data_type": "boolean"}
        ],
        "retrieval_options": {"boost_by": [
            {"field": "priority", "direction": "desc"},
            {"field": "DRAFT", "direction": "not_exists"},
            {"field": "timestamp"}
        ]}
    }))
    .unwrap();
    let prepared = input.prepare(&catalog()).unwrap();
    let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
    assert_eq!(public["reranking_model"], "@cf/baai/bge-reranker-base");
    assert_eq!(
        public["retrieval_options"]["boost_by"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    for boost_by in [
        serde_json::json!([{"field":"missing"}]),
        serde_json::json!([{"field":"draft","direction":"desc"}]),
        serde_json::json!([{"field":"Priority"},{"field":"priority"}]),
    ] {
        let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
            "id": "bad",
            "index_method": {"vector": false, "keyword": true},
            "custom_metadata": [
                {"field_name": "Priority", "data_type": "number"},
                {"field_name": "draft", "data_type": "boolean"}
            ],
            "retrieval_options": {"boost_by": boost_by}
        }))
        .unwrap();
        assert!(input.prepare(&catalog()).is_err());
    }
}

#[test]
fn plugin_origin_metadata_round_trips_and_rejects_untyped_shapes() {
    let prepare = |origin: Value| {
        let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
            "id": "origin",
            "metadata": {"created_from_emdash_plugin": origin, "owner": "docs"}
        }))
        .unwrap();
        input.prepare(&catalog())
    };
    for origin in [
        serde_json::json!({"type": "native", "version": "1.0.0"}),
        serde_json::json!({"type": "rest", "version": "2", "extra": true}),
    ] {
        let prepared = prepare(origin.clone()).unwrap();
        let public: Value = serde_json::from_slice(&prepared.public_config_json).unwrap();
        assert_eq!(public["metadata"]["created_from_emdash_plugin"], origin);
    }
    for origin in [
        serde_json::json!("rest"),
        serde_json::json!({"type": "cli", "version": "1"}),
        serde_json::json!({"type": "rest"}),
        serde_json::json!({"type": "rest", "version": 1}),
    ] {
        assert!(prepare(origin.clone()).is_err(), "{origin}");
    }
}
