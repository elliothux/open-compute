use super::*;

#[test]
fn custom_ai_validates_bounded_models_and_credentials_without_exposing_them() {
    let custom = json!({"model":"openai/gpt-4o","authorization":"Bearer request-key"});
    assert!(validate_options(&json!({"custom_ai":[custom.clone()]})).is_ok());
    assert!(
        validate_options(&json!({"custom_ai":[{"model":"workers-ai/@cf/meta/model"}]})).is_ok()
    );
    assert!(validate_options(&json!({"custom_ai":[{"model":"openai/gpt-4o"}]})).is_err());
    assert!(validate_options(&json!({"custom_ai":[{"model":"workers-ai/@cf/meta/model","authorization":"request-key"}]})).is_ok());
    for value in [
        json!([]),
        Value::Null,
        json!(vec![custom; 4]),
        json!([{"model":"openai/gpt-4o","authorization":"key","endpoint":"https://other.example/chat"}]),
    ] {
        assert_eq!(
            validate_options(&json!({"custom_ai":value}))
                .unwrap_err()
                .code(),
            ErrorCode::BrowserInputInvalid
        );
    }
    for model in [
        "tenant",
        "/model",
        "openai/",
        "openai/a b",
        "!provider/model",
        &format!("openai/{}", "x".repeat(256)),
    ] {
        assert!(
            validate_options(&json!({"custom_ai":[{"model":model,"authorization":"key"}]}))
                .is_err()
        );
    }
    for authorization in [
        "",
        "Bearer ",
        "Bearer a b",
        "key\r\nInjected: value",
        "é",
        &"x".repeat(4097),
    ] {
        let error = validate_options(
            &json!({"custom_ai":[{"model":"openai/model","authorization":authorization}]}),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::BrowserInputInvalid);
        assert_eq!(error.message(), "browser request is invalid");
    }
}

#[test]
fn extraction_schema_enforces_shape_and_rejects_host_references_and_unbounded_work() {
    let format: Format = serde_json::from_value(json!({"type":"json_schema","json_schema":{
        "type":"object","properties":{"heading":{"type":"string","pattern":"^rendered$"}},
        "required":["heading"],"additionalProperties":false
    }}))
    .unwrap();
    let validator = schema(&format).unwrap().unwrap();
    assert!(validator.is_valid(&json!({"heading":"rendered"})));
    for result in [
        json!({"heading":42}),
        json!({}),
        json!({"heading":"other"}),
        json!({"heading":"rendered","extra":true}),
    ] {
        assert!(!validator.is_valid(&result));
    }
    assert!(schema(&Format::JsonObject).unwrap().is_none());
    for value in [
        json!({"$ref":"file:///etc/passwd"}),
        json!({"$dynamicRef":"https://localhost/private"}),
        json!({"properties":{"x":{"$ref":"https://localhost/private"}}}),
    ] {
        assert_eq!(
            schema(&Format::JsonSchema { json_schema: value })
                .unwrap_err()
                .code(),
            ErrorCode::BrowserUnsupported
        );
    }
    for value in [
        json!(true),
        json!({"type":42}),
        json!({"pattern":"["}),
        json!({"$ref":42}),
        json!({"$ref":"#/$defs/missing"}),
        json!({"description":"x".repeat(32769)}),
    ] {
        assert_eq!(
            schema(&Format::JsonSchema { json_schema: value })
                .unwrap_err()
                .code(),
            ErrorCode::BrowserInputInvalid
        );
    }
    let mut deep = json!({"type":"string"});
    for _ in 0..17 {
        deep = json!({"properties":{"x":deep}});
    }
    for value in [deep, json!({"enum":vec![1;257]})] {
        assert_eq!(
            schema(&Format::JsonSchema { json_schema: value })
                .unwrap_err()
                .code(),
            ErrorCode::BrowserLimitExceeded
        );
    }
    assert!(validate_options(&json!({"prompt":"extract"})).is_ok());
    assert!(validate_options(&json!({"response_format":{"type":"json_object"}})).is_ok());
    assert!(validate_options(&json!({"response_format":{"type":"unknown"}})).is_err());
}

#[test]
fn extraction_schema_validates_local_definitions_anchors_and_recursive_instances_offline() {
    for definition in [
        json!({"$defs":{"heading":{"type":"string","pattern":"^rendered$"}},"type":"object","properties":{"heading":{"$ref":"#/$defs/heading"}},"required":["heading"],"additionalProperties":false}),
        json!({"$defs":{"heading":{"$anchor":"heading","type":"string","pattern":"^rendered$"}},"type":"object","properties":{"heading":{"$ref":"#heading"}},"required":["heading"],"additionalProperties":false}),
    ] {
        let validator = schema(&Format::JsonSchema {
            json_schema: definition,
        })
        .unwrap()
        .unwrap();
        assert!(validator.is_valid(&json!({"heading":"rendered"})));
        assert!(!validator.is_valid(&json!({"heading":42})));
        assert!(!validator.is_valid(&json!({"heading":"other"})));
    }
    let recursive = schema(&Format::JsonSchema { json_schema: json!({
        "$defs":{"node":{"type":"object","properties":{"value":{"type":"integer"},"next":{"$ref":"#/$defs/node"}},"required":["value"]}},"$ref":"#/$defs/node"
    }) }).unwrap().unwrap();
    assert!(recursive.is_valid(&json!({"value":1,"next":{"value":2}})));
    assert!(!recursive.is_valid(&json!({"value":1,"next":{"value":"wrong"}})));
    let cyclic = schema(&Format::JsonSchema {
        json_schema: json!({
            "$defs":{"a":{"$ref":"#/$defs/b"},"b":{"$ref":"#/$defs/a"}},"$ref":"#/$defs/a"
        }),
    })
    .unwrap()
    .unwrap();
    assert!(cyclic.is_valid(&json!({"value":"finite"})));
    let escaped = schema(&Format::JsonSchema {
        json_schema: json!({
            "$defs":{"a/b":{"type":"integer"}},"$ref":"#/$defs/a~1b"
        }),
    })
    .unwrap()
    .unwrap();
    assert!(escaped.is_valid(&json!(7)));
    assert!(!escaped.is_valid(&json!("seven")));
    assert!(
        jsonschema::Retrieve::retrieve(
            &NoSchemaRetrieval,
            &jsonschema::Uri::parse("file:///private/authority.json".to_owned()).unwrap()
        )
        .is_err()
    );
}

#[test]
fn provider_failures_remain_content_free_and_have_stable_browser_classes() {
    for (error, expected) in [
        (AiProviderError::Timeout, ErrorCode::BrowserTimeout),
        (
            AiProviderError::RateLimited {
                retry_after_seconds: Some(5),
            },
            ErrorCode::BrowserLimitExceeded,
        ),
        (
            AiProviderError::InvalidRequest,
            ErrorCode::BrowserInputInvalid,
        ),
        (AiProviderError::Unauthorized, ErrorCode::BrowserUnavailable),
        (
            AiProviderError::MalformedResponse,
            ErrorCode::BrowserUnavailable,
        ),
    ] {
        assert_eq!(provider_error(error).code(), expected);
    }
}

#[tokio::test]
async fn custom_ai_fallback_keeps_order_credentials_schema_and_one_total_deadline() {
    use axum::http::{HeaderMap, StatusCode};
    use axum::{Json, Router, routing::post};
    use open_compute_core::{
        AiAuthConfig, AiBackendConfig, AiBackendProtocol, AiConfig, AiGenerationCapability,
        AiGenerationModelConfig,
    };

    let requests = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let observed = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/chat", post(move |headers: HeaderMap, Json(body): Json<Value>| {
        let observed = observed.clone();
        async move {
            let model = body["model"].as_str().unwrap();
            observed.lock().unwrap().push((model.into(), headers.get("authorization").map_or("", |v| v.to_str().unwrap()).into()));
            let content = match model {
                "openai/denied" => return StatusCode::UNAUTHORIZED.into_response(),
                "openai/rate" => return StatusCode::TOO_MANY_REQUESTS.into_response(),
                "openai/malformed" => "not JSON".to_owned(),
                "openai/schema" => "{\"other\":1}".to_owned(),
                "openai/array" => "[]".to_owned(),
                "openai/large" => "x".repeat(4097),
                "openai/slow" => return std::future::pending::<Response>().await,
                _ => "{\"heading\":\"rendered\"}".to_owned(),
            };
            Json(json!({"model":model,"choices":[{"index":0,"message":{"role":"assistant","content":content},"finish_reason":if model=="openai/finish" {"length"} else {"stop"}}]})).into_response()
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &open_compute_core::DataConfig {
                path: data.clone(),
                master_key_file: data.join("keys/master.key"),
                ..Default::default()
            },
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let mut ai = AiConfig::default();
    ai.backends.insert(
        "chat".into(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiChatCompletionsV1,
            endpoint: format!("http://{address}/chat"),
            auth: AiAuthConfig::None,
            headers: BTreeMap::new(),
        },
    );
    for name in [
        "denied",
        "rate",
        "malformed",
        "schema",
        "array",
        "limited",
        "large",
        "finish",
        "ok",
        "unused",
        "slow",
    ] {
        let alias = format!("openai/{name}");
        ai.generation_models.insert(
            alias.clone(),
            AiGenerationModelConfig {
                backend: "chat".into(),
                remote_model: alias,
                provider_revision: None,
                max_context_tokens: if name == "limited" { 1 } else { 8192 },
                capabilities: BTreeSet::from([AiGenerationCapability::Chat]),
            },
        );
    }
    let mut config = super::super::tests::config("http://127.0.0.1:9/json/version".into());
    ai.generation_models.insert(
        "workers-ai/@cf/meta/model".into(),
        ai.generation_models["openai/ok"].clone(),
    );
    config.command_timeout_ms = 2000;
    let service = BrowserService::new(
        storage.clone(),
        config.clone(),
        None,
        ai.clone(),
        None,
        crate::browser::tests::metrics(),
    )
    .unwrap();
    let format = Format::JsonSchema {
        json_schema: json!({"type":"object","required":["heading"],"properties":{"heading":{"type":"string"}}}),
    };
    let validator = schema(&format).unwrap().unwrap();
    let messages = [ChatMessage::user("extract")];
    assert_eq!(
        service
            .generate_json(
                Some(&[CustomAi {
                    model: "workers-ai/@cf/meta/model".into(),
                    authorization: None
                }]),
                &messages,
                32,
                None,
                None
            )
            .await
            .unwrap(),
        json!({"heading":"rendered"})
    );
    assert_eq!(
        requests.lock().unwrap().pop().unwrap(),
        ("openai/ok".into(), "".into())
    );
    for name in [
        "unknown/model",
        "openai/limited",
        "openai/denied",
        "openai/rate",
        "openai/malformed",
        "openai/schema",
        "openai/large",
        "openai/finish",
    ] {
        let models: Vec<CustomAi> = [name, "openai/ok", "openai/unused"]
            .iter()
            .map(|name| CustomAi {
                model: (*name).into(),
                authorization: Some(format!("key-{}", name.replace('/', "-"))),
            })
            .collect();
        assert_eq!(
            service
                .generate_json(
                    Some(&models),
                    &messages,
                    32,
                    Some(&format),
                    Some(&validator)
                )
                .await
                .unwrap(),
            json!({"heading":"rendered"})
        );
    }
    let actual = requests.lock().unwrap().clone();
    let expected = [
        "openai/ok",
        "openai/ok",
        "openai/denied",
        "openai/ok",
        "openai/rate",
        "openai/ok",
        "openai/malformed",
        "openai/ok",
        "openai/schema",
        "openai/ok",
        "openai/large",
        "openai/ok",
        "openai/finish",
        "openai/ok",
    ];
    assert_eq!(
        actual.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        expected
    );
    for (model, credential) in actual {
        assert_eq!(
            credential,
            format!("Bearer key-{}", model.replace('/', "-"))
        );
    }

    assert_eq!(
        service
            .generate_json(
                Some(&[CustomAi {
                    model: "openai/array".into(),
                    authorization: Some("key".into())
                }]),
                &messages,
                32,
                Some(&Format::JsonObject),
                None
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BrowserUnavailable
    );
    let denied = [CustomAi {
        model: "openai/rate".into(),
        authorization: Some("bad-key".into()),
    }];
    assert_eq!(
        service
            .generate_json(Some(&denied), &messages, 32, None, None)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BrowserLimitExceeded
    );
    assert_eq!(
        service
            .generate_json(None, &messages, 32, None, None)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BrowserUnavailable
    );
    ai.default_generation_model = Some("openai/ok".into());
    let default = BrowserService::new(
        storage.clone(),
        config.clone(),
        None,
        ai.clone(),
        None,
        crate::browser::tests::metrics(),
    )
    .unwrap();
    assert_eq!(
        default
            .generate_json(None, &messages, 32, None, None)
            .await
            .unwrap(),
        json!({"heading":"rendered"})
    );
    assert_eq!(requests.lock().unwrap().last().unwrap().1, "");
    let count = requests.lock().unwrap().len();
    ai.query_timeout_ms = 5_000;
    config.command_timeout_ms = 10_000;
    let bounded = BrowserService::new(
        storage,
        config,
        None,
        ai,
        None,
        crate::browser::tests::metrics(),
    )
    .unwrap();
    let slow = [
        CustomAi {
            model: "openai/slow".into(),
            authorization: Some("key".into()),
        },
        CustomAi {
            model: "openai/unused".into(),
            authorization: Some("other".into()),
        },
    ];
    let started = Instant::now();
    assert_eq!(
        bounded
            .generate_json(Some(&slow), &messages, 32, None, None)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BrowserTimeout
    );
    assert!(started.elapsed() < Duration::from_secs(6));
    assert_eq!(requests.lock().unwrap().len(), count + 1);
    service.shutdown().await.unwrap();
    default.shutdown().await.unwrap();
    bounded.shutdown().await.unwrap();
    server.abort();
}
