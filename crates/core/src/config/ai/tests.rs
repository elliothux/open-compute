use super::*;
use crate::SecretReference;

const ALIAS: &str = "@cf/qwen/qwen3-embedding-0.6b";
const PROFILE: &str = "fixture/qwen3-1024";

fn secret() -> SecretReference {
    SecretReference {
        env: Some("AI_FIXTURE_KEY".to_owned()),
        file: None,
    }
}

fn embedding_profile() -> AiEmbeddingProfileConfig {
    AiEmbeddingProfileConfig {
        dimensions: 1_024,
        max_input_tokens: 8_192,
        send_dimensions: false,
        tokenizer: AiTokenizerConfig {
            kind: AiTokenizer::Qwen3,
            revision: "tokenizer-revision".to_owned(),
            artifact: AiTokenizerArtifactConfig {
                path: PathBuf::from("/opt/open-compute/models/qwen3/tokenizer.json"),
                sha256: "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"
                    .to_owned(),
            },
        },
    }
}

fn configured() -> AiConfig {
    let mut config = AiConfig::default();
    config.backends.insert(
        "fixture-embeddings".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiEmbeddingsV1,
            endpoint: "http://127.0.0.1:8080/compatible-mode/v1/embeddings".to_owned(),
            auth: AiAuthConfig::Bearer { secret: secret() },
            headers: BTreeMap::from([("X-Title".to_owned(), "open-compute".to_owned())]),
        },
    );
    config
        .embedding_profiles
        .insert(PROFILE.to_owned(), embedding_profile());
    config.embedding_models.insert(
        ALIAS.to_owned(),
        AiEmbeddingModelConfig {
            backend: "fixture-embeddings".to_owned(),
            remote_model: "qwen3.7-text-embedding-flash".to_owned(),
            provider_revision: None,
            profile: PROFILE.to_owned(),
        },
    );
    config.default_embedding_model = Some(ALIAS.to_owned());
    config
}

fn add_chat(config: &mut AiConfig) {
    config.backends.insert(
        "fixture-chat".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiChatCompletionsV1,
            endpoint: "http://127.0.0.1:8080/compatible-mode/v1/chat/completions".to_owned(),
            auth: AiAuthConfig::None,
            headers: BTreeMap::new(),
        },
    );
    config.generation_models.insert(
        "fixture/chat".to_owned(),
        AiGenerationModelConfig {
            backend: "fixture-chat".to_owned(),
            remote_model: "fixture-chat".to_owned(),
            provider_revision: Some("revision-1".to_owned()),
            max_context_tokens: 8_192,
            capabilities: [
                AiGenerationCapability::Chat,
                AiGenerationCapability::Rewrite,
            ]
            .into_iter()
            .collect(),
        },
    );
}

fn add_vlm(config: &mut AiConfig) {
    add_chat(config);
    config.vlm_models.insert(
        "fixture/vision".to_owned(),
        AiVlmModelConfig {
            backend: "fixture-chat".to_owned(),
            remote_model: "fixture-vision".to_owned(),
            provider_revision: Some("vision-revision-1".to_owned()),
            max_input_width: 1_280,
            max_input_height: 720,
            max_input_pixels: 921_600,
            max_encoded_image_bytes: 4 * 1024 * 1024,
            max_output_tokens: 1_024,
        },
    );
    config.default_vlm_model = Some("fixture/vision".to_owned());
}

#[test]
fn vlm_contract_is_optional_bounded_and_secret_free() {
    assert!(
        AiConfig::default()
            .resolve_default_vlm_model()
            .unwrap()
            .is_none()
    );
    let mut config = configured();
    add_vlm(&mut config);
    let contract = config
        .resolve_default_vlm_model()
        .unwrap()
        .expect("configured VLM");
    assert_eq!(contract.max_input_width, 1_280);
    assert_eq!(contract.max_images_per_document, 16);
    assert_eq!(contract.max_request_bytes, 8 * 1024 * 1024);
    assert_eq!(contract.max_response_bytes, 1024 * 1024);
    assert_eq!(contract.protocol, "openai_chat_completions_v1");
    assert!(!contract.contract_sha256.is_empty());
    let json = serde_json::to_string(&contract).unwrap();
    assert!(!json.contains("AI_FIXTURE_KEY"));
    assert!(!json.contains("127.0.0.1"));

    for mutate in [0_u8, 1, 2, 3, 4] {
        let mut invalid = config.clone();
        let model = invalid.vlm_models.get_mut("fixture/vision").unwrap();
        match mutate {
            0 => model.max_input_width = 0,
            1 => model.max_input_height = 8_193,
            2 => model.max_input_pixels = 16_777_217,
            3 => model.max_encoded_image_bytes = 16 * 1024 * 1024 + 1,
            _ => model.max_output_tokens = 4_097,
        }
        assert!(invalid.validate().is_err());
    }

    let mut incompatible = config;
    incompatible
        .vlm_models
        .get_mut("fixture/vision")
        .unwrap()
        .backend = "fixture-embeddings".to_owned();
    assert!(incompatible.validate().is_err());

    let mut too_many_images = configured();
    too_many_images.max_vlm_images_per_document = 17;
    assert!(too_many_images.validate().is_err());
}

#[test]
fn catalog_resolves_prefixed_endpoint_profile_to_stable_secret_free_contract() {
    let config = configured();
    config.validate().unwrap();
    let first = config.resolve_embedding_model(None).unwrap();
    let second = config.resolve_embedding_model(Some(ALIAS)).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.dimensions, 1_024);
    assert_eq!(first.auth_kind, "bearer");
    assert_eq!(first.profile, PROFILE);
    assert_eq!(first.metric, AiEmbeddingMetric::Cosine);
    assert!(!first.send_dimensions);
    let tokenizer = config.resolve_tokenizer(None).unwrap();
    assert_eq!(tokenizer.tokenizer, AiTokenizer::Qwen3);
    assert_eq!(tokenizer.profile, PROFILE);
    assert_eq!(tokenizer.max_input_tokens, 8_192);
    assert_ne!(tokenizer.contract_sha256, first.contract_sha256);
    let json = serde_json::to_string(&first).unwrap();
    assert!(!json.contains("AI_FIXTURE_KEY"));
    assert!(!json.contains("127.0.0.1"));
    assert!(!json.contains("open-compute"));
}

#[test]
fn backend_profile_and_catalog_drift_fail_closed() {
    let mut config = configured();
    config
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .endpoint = "http://example.com/v1/embeddings".to_owned();
    assert!(config.validate().is_err());

    let mut config = configured();
    config
        .embedding_profiles
        .get_mut(PROFILE)
        .unwrap()
        .dimensions = 0;
    assert!(config.validate().is_err());

    let mut config = configured();
    config.embedding_models.get_mut(ALIAS).unwrap().profile = "missing/profile".into();
    assert!(config.validate().is_err());

    let mut config = configured();
    config.default_embedding_model = Some("missing/model".to_owned());
    assert!(config.validate().is_err());

    let mut config = configured();
    config
        .embedding_profiles
        .get_mut(PROFILE)
        .unwrap()
        .tokenizer
        .artifact
        .path = PathBuf::from("relative/tokenizer.json");
    assert!(config.validate().is_err());
}

#[test]
fn endpoint_auth_and_header_contracts_are_closed() {
    let mut config = configured();
    config
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .endpoint = "https://api.example.com/v1/embeddings".to_owned();
    config.backends.get_mut("fixture-embeddings").unwrap().auth = AiAuthConfig::None;
    assert!(config.validate().is_err());
    config.backends.get_mut("fixture-embeddings").unwrap().auth =
        AiAuthConfig::Bearer { secret: secret() };
    config.validate().unwrap();

    for endpoint in [
        "not-a-url",
        "ftp://example.com/v1/embeddings",
        "https://user@example.com/v1/embeddings",
        "https://example.com/v1/embeddings?x=1",
        "https://example.com/v1/embeddings#fragment",
        "https://example.com/a/../v1/embeddings",
        "https://example.com/",
        "https://example.com/v1/embeddings/",
        "http://[::2]/v1/embeddings",
    ] {
        let mut invalid = configured();
        invalid
            .backends
            .get_mut("fixture-embeddings")
            .unwrap()
            .endpoint = endpoint.to_owned();
        assert!(invalid.validate().is_err(), "{endpoint}");
    }

    let mut custom_auth = configured();
    custom_auth
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .auth = AiAuthConfig::Header {
        name: "X-API-Key".to_owned(),
        secret: secret(),
    };
    custom_auth.validate().unwrap();
    let contract = custom_auth.resolve_embedding_model(None).unwrap();
    assert_eq!(contract.auth_kind, "header");
    assert_eq!(contract.auth_header_name.as_deref(), Some("x-api-key"));

    for name in [
        "Authorization",
        "Host",
        "Content-Type",
        "Proxy-Test",
        "bad name",
    ] {
        let mut invalid = configured();
        invalid
            .backends
            .get_mut("fixture-embeddings")
            .unwrap()
            .headers = BTreeMap::from([(name.to_owned(), "value".to_owned())]);
        assert!(invalid.validate().is_err(), "{name}");
    }

    let mut duplicate = configured();
    duplicate
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .headers = BTreeMap::from([
        ("X-Title".to_owned(), "one".to_owned()),
        ("x-title".to_owned(), "two".to_owned()),
    ]);
    assert!(duplicate.validate().is_err());
}

#[test]
fn limits_names_profiles_and_generation_protocols_are_validated() {
    let invalid_limits = [
        ("max_provider_in_flight", 0_u64),
        ("max_provider_in_flight", 257),
        ("max_embedding_inputs_per_batch", 0),
        ("max_embedding_inputs_per_batch", 513),
        ("max_embedding_request_bytes", 0),
        ("max_embedding_response_bytes", 0),
        ("provider_timeout_ms", 0),
        ("query_timeout_ms", 0),
    ];
    for (field, value) in invalid_limits {
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
            ErrorCode::LimitInvalid
        );
    }

    let mut config = configured();
    config.provider_timeout_ms = 99;
    config.query_timeout_ms = 100;
    assert_eq!(
        config.validate().unwrap_err().code(),
        ErrorCode::LimitInvalid
    );

    for name in ["", "9backend", "bad backend", &"x".repeat(65)] {
        let mut config = configured();
        let backend = config.backends.remove("fixture-embeddings").unwrap();
        config.backends.insert(name.to_owned(), backend);
        assert!(config.validate().is_err(), "{name}");
    }

    let mut config = configured();
    add_chat(&mut config);
    config.default_generation_model = Some("fixture/chat".to_owned());
    config.validate().unwrap();
    config
        .generation_models
        .get_mut("fixture/chat")
        .unwrap()
        .backend = "fixture-embeddings".to_owned();
    assert!(config.validate().is_err());
}

#[test]
fn profile_changes_are_frozen_and_old_provider_shape_is_rejected() {
    let config = configured();
    let original = config.resolve_embedding_model(None).unwrap();
    let mut changed = config.clone();
    changed
        .embedding_profiles
        .get_mut(PROFILE)
        .unwrap()
        .send_dimensions = true;
    let changed = changed.resolve_embedding_model(None).unwrap();
    assert_ne!(
        original.profile_contract_sha256,
        changed.profile_contract_sha256
    );
    assert_ne!(original.contract_sha256, changed.contract_sha256);

    let mut header_case = config.clone();
    header_case
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .headers = BTreeMap::from([
        ("a-header".to_owned(), "first".to_owned()),
        ("Z-Header".to_owned(), "last".to_owned()),
    ]);
    let first = header_case.resolve_embedding_model(None).unwrap();
    header_case
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .headers = BTreeMap::from([
        ("A-HEADER".to_owned(), "first".to_owned()),
        ("z-header".to_owned(), "last".to_owned()),
    ]);
    let second = header_case.resolve_embedding_model(None).unwrap();
    assert_eq!(first.headers_sha256, second.headers_sha256);
    assert_eq!(first.contract_sha256, second.contract_sha256);

    let mut rotated_secret = config.clone();
    rotated_secret
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .auth = AiAuthConfig::Bearer {
        secret: SecretReference {
            env: Some("ROTATED_FIXTURE_KEY".to_owned()),
            file: None,
        },
    };
    assert_eq!(
        original.contract_sha256,
        rotated_secret
            .resolve_embedding_model(None)
            .unwrap()
            .contract_sha256
    );

    let mut routed_header = config.clone();
    routed_header
        .backends
        .get_mut("fixture-embeddings")
        .unwrap()
        .headers
        .insert("X-Title".to_owned(), "different-route".to_owned());
    assert_ne!(
        original.contract_sha256,
        routed_header
            .resolve_embedding_model(None)
            .unwrap()
            .contract_sha256
    );

    let old = r#"
        [providers.fixture]
        base_url = "http://127.0.0.1:8080/v1"
        auth = { kind = "none" }
    "#;
    assert!(toml::from_str::<AiConfig>(old).is_err());
}
