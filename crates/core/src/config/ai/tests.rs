use super::*;

fn configured() -> AiConfig {
    let mut config = AiConfig::default();
    config.providers.insert(
        "fixture".to_owned(),
        AiProviderConfig {
            base_url: "http://127.0.0.1:8080/v1".to_owned(),
            auth: AiAuthConfig::Bearer {
                secret: SecretReference {
                    env: Some("AI_FIXTURE_KEY".to_owned()),
                    file: None,
                },
            },
        },
    );
    config.embedding_models.insert(
        "@cf/qwen/qwen3-embedding-0.6b".to_owned(),
        AiEmbeddingModelConfig {
            provider: "fixture".to_owned(),
            remote_model: "@cf/qwen/qwen3-embedding-0.6b".to_owned(),
            model_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
            dimensions: 1024,
            request_dimensions: None,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 8192,
            tokenizer: AiTokenizer::Qwen3,
            tokenizer_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
            tokenizer_artifact: AiTokenizerArtifactConfig {
                path: PathBuf::from("/opt/open-compute/models/qwen3/tokenizer.json"),
                sha256: "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"
                    .to_owned(),
            },
        },
    );
    config.default_embedding_model = Some("@cf/qwen/qwen3-embedding-0.6b".to_owned());
    config
}

#[test]
fn fixture_catalog_resolves_to_stable_secret_free_contract() {
    let config = configured();
    config.validate().unwrap();
    let first = config.resolve_embedding_model(None).unwrap();
    let second = config
        .resolve_embedding_model(Some("@cf/qwen/qwen3-embedding-0.6b"))
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.dimensions, 1024);
    assert_eq!(first.auth_kind, "bearer");
    let tokenizer = config.resolve_tokenizer(None).unwrap();
    assert_eq!(tokenizer.tokenizer, AiTokenizer::Qwen3);
    assert_eq!(tokenizer.max_input_tokens, 8192);
    assert_ne!(tokenizer.contract_sha256, first.contract_sha256);
    let json = serde_json::to_string(&first).unwrap();
    assert!(!json.contains("AI_FIXTURE_KEY"));
    assert!(!json.contains("127.0.0.1"));
}

#[test]
fn provider_and_catalog_drift_fail_closed() {
    let mut config = configured();
    config.providers.get_mut("fixture").unwrap().base_url = "http://example.com/v1".to_owned();
    assert!(config.validate().is_err());

    let mut config = configured();
    config
        .embedding_models
        .get_mut("@cf/qwen/qwen3-embedding-0.6b")
        .unwrap()
        .dimensions = 768;
    assert!(config.validate().is_err());

    let mut config = configured();
    config.default_embedding_model = Some("missing/model".to_owned());
    assert!(config.validate().is_err());

    let mut config = configured();
    config
        .embedding_models
        .get_mut("@cf/qwen/qwen3-embedding-0.6b")
        .unwrap()
        .tokenizer_artifact
        .path = PathBuf::from("relative/tokenizer.json");
    assert!(config.validate().is_err());

    let mut config = configured();
    config
        .embedding_models
        .get_mut("@cf/qwen/qwen3-embedding-0.6b")
        .unwrap()
        .tokenizer_artifact
        .sha256 = "ABC".repeat(21);
    assert!(config.validate().is_err());
}

#[test]
fn anonymous_auth_is_only_for_loopback_http() {
    let mut config = configured();
    config.providers.get_mut("fixture").unwrap().base_url = "https://api.example.com/v1".to_owned();
    config.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::None;
    assert!(config.validate().is_err());
    config.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::Bearer {
        secret: SecretReference {
            env: Some("AI_FIXTURE_KEY".to_owned()),
            file: None,
        },
    };
    config.validate().unwrap();
}

#[test]
fn every_operator_limit_and_timeout_relationship_is_bounded() {
    let invalid = [
        ("max_provider_in_flight", 0_u64),
        ("max_provider_in_flight", 257),
        ("max_embedding_inputs_per_batch", 0),
        ("max_embedding_inputs_per_batch", 513),
        ("max_embedding_request_bytes", 0),
        ("max_embedding_request_bytes", 16 * 1024 * 1024 + 1),
        ("max_embedding_response_bytes", 0),
        ("max_embedding_response_bytes", 256 * 1024 * 1024 + 1),
        ("provider_timeout_ms", 0),
        ("provider_timeout_ms", 300_001),
        ("query_timeout_ms", 0),
        ("query_timeout_ms", 300_001),
    ];
    for (field, value) in invalid {
        let mut config = configured();
        match field {
            "max_provider_in_flight" => config.max_provider_in_flight = value as u16,
            "max_embedding_inputs_per_batch" => {
                config.max_embedding_inputs_per_batch = value as u16;
            }
            "max_embedding_request_bytes" => config.max_embedding_request_bytes = value,
            "max_embedding_response_bytes" => config.max_embedding_response_bytes = value,
            "provider_timeout_ms" => config.provider_timeout_ms = value,
            "query_timeout_ms" => config.query_timeout_ms = value,
            _ => unreachable!(),
        }
        assert_eq!(
            config.validate().unwrap_err().code(),
            ErrorCode::LimitInvalid,
            "{field}"
        );
    }
    let mut config = configured();
    config.provider_timeout_ms = 99;
    config.query_timeout_ms = 100;
    assert_eq!(
        config.validate().unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
}

#[test]
fn provider_urls_names_aliases_and_model_text_are_canonical() {
    for url in [
        "not-a-url",
        "ftp://example.com/v1",
        "https://user@example.com/v1",
        "https://example.com/v1?x=1",
        "https://example.com/v1#fragment",
        "https://example.com/other",
        "http://[::2]/v1",
    ] {
        let mut config = configured();
        config.providers.get_mut("fixture").unwrap().base_url = url.to_string();
        assert!(config.validate().is_err(), "{url}");
    }
    let mut loopback = configured();
    loopback.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::None;
    loopback.validate().unwrap();
    assert_eq!(AiAuthConfig::None.kind_token(), "none");
    assert_eq!(
        configured().providers["fixture"].auth.kind_token(),
        "bearer"
    );

    for name in ["", "9provider", "bad provider", &"x".repeat(65)] {
        let mut config = configured();
        let provider = config.providers.remove("fixture").unwrap();
        config.providers.insert(name.to_string(), provider);
        assert!(config.validate().is_err(), "{name}");
    }
    for alias in [
        "missing-slash",
        " bad/model",
        "bad /model",
        &"x".repeat(257),
    ] {
        let mut config = configured();
        let model = config
            .embedding_models
            .remove("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap();
        config.default_embedding_model = None;
        config.embedding_models.insert(alias.to_string(), model);
        assert!(config.validate().is_err(), "{alias}");
    }
    for field in ["remote_model", "model_revision", "tokenizer_revision"] {
        let mut config = configured();
        let model = config
            .embedding_models
            .get_mut("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap();
        match field {
            "remote_model" => model.remote_model = " trailing ".to_string(),
            "model_revision" => model.model_revision.clear(),
            "tokenizer_revision" => model.tokenizer_revision = "bad\nrevision".to_string(),
            _ => unreachable!(),
        }
        assert!(config.validate().is_err(), "{field}");
    }
}

#[test]
fn catalog_resolution_and_generation_validation_reject_drift() {
    let empty = AiConfig::default();
    assert!(empty.resolve_embedding_model(None).is_err());
    assert!(empty.resolve_tokenizer(None).is_err());
    let config = configured();
    assert!(
        config
            .resolve_embedding_model(Some("missing/model"))
            .is_err()
    );
    assert!(config.resolve_tokenizer(Some("missing/model")).is_err());

    for mutate in 0..5 {
        let mut config = configured();
        let model = config
            .embedding_models
            .get_mut("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap();
        match mutate {
            0 => model.provider = "missing".to_string(),
            1 => model.max_input_tokens = 512,
            2 => model.request_dimensions = Some(768),
            3 => model.tokenizer_artifact.path = PathBuf::from("/opt/../tokenizer.json"),
            4 => model.tokenizer_artifact.sha256 = "g".repeat(64),
            _ => unreachable!(),
        }
        assert!(config.validate().is_err(), "mutation {mutate}");
    }

    let generation = AiGenerationModelConfig {
        provider: "fixture".to_string(),
        remote_model: "fixture-chat".to_string(),
        model_revision: "revision-1".to_string(),
        max_context_tokens: 8_192,
        capabilities: [
            AiGenerationCapability::Chat,
            AiGenerationCapability::Rewrite,
        ]
        .into_iter()
        .collect(),
    };
    let mut valid = configured();
    valid
        .generation_models
        .insert("fixture/chat".to_string(), generation.clone());
    valid.default_generation_model = Some("fixture/chat".to_string());
    valid.validate().unwrap();

    for mutate in 0..5 {
        let mut config = configured();
        let mut model = generation.clone();
        match mutate {
            0 => model.provider = "missing".to_string(),
            1 => model.max_context_tokens = 0,
            2 => model.max_context_tokens = 4_000_001,
            3 => model.capabilities.clear(),
            4 => model.remote_model = "".to_string(),
            _ => unreachable!(),
        }
        config
            .generation_models
            .insert("fixture/chat".to_string(), model);
        assert!(config.validate().is_err(), "generation mutation {mutate}");
    }
    let mut config = configured();
    config.default_generation_model = Some("missing/chat".to_string());
    assert!(config.validate().is_err());
}
