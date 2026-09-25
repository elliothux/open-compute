use super::*;

#[tokio::test]
async fn vendor_capabilities_and_system_status_use_the_canonical_envelope() {
    let (state, _) = state();
    let state = state.with_capability_limits(std::collections::BTreeMap::from([(
        "workers.max_scripts_per_account".to_owned(),
        42,
    )]));
    let capabilities = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/open-compute/capabilities")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(capabilities.status(), StatusCode::OK);
    let capabilities = json(capabilities).await;
    assert_eq!(capabilities["success"], true);
    assert_eq!(capabilities["result"]["wrangler_version"], "4.138.0");
    assert_eq!(
        capabilities["result"]["compatibility_date"]["minimum"],
        "2026-09-08"
    );
    assert_eq!(
        capabilities["result"]["compatibility_flags"],
        serde_json::json!(["nodejs_compat"])
    );
    assert_eq!(
        capabilities["result"]["limits"]["workers.max_scripts_per_account"],
        42
    );
    assert_eq!(capabilities["result"]["configuration"]["ai_search"], false);
    let endpoint_count = capabilities["result"]["endpoints"]
        .as_object()
        .unwrap()
        .len();
    let authority_count = serde_json::from_slice::<serde_json::Value>(include_bytes!(
        "../../../../../openapi/p6-capability.json"
    ))
    .unwrap()["managementApi"]["routes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(endpoint_count, authority_count);
    let deviations = capabilities["result"]["deviations"]
        .as_array()
        .expect("deviations");
    assert_eq!(deviations[0], "OC-ACCOUNT-SUBDOMAIN-001");
    assert!(
        deviations
            .iter()
            .any(|value| value == "OC-OBSERVABILITY-001"),
        "{deviations:?}"
    );
    assert!(
        deviations
            .iter()
            .any(|value| value == "OC-MANAGEMENT-COMPATIBILITY-DATE-001"),
        "{deviations:?}"
    );

    let status = app(state)
        .oneshot(
            Request::builder()
                .uri("/open-compute/system/status")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status = json(status).await;
    assert_eq!(status["success"], true);
    assert!(status["result"]["components"].as_array().unwrap().len() > 5);
}
