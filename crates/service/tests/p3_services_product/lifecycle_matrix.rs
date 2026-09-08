use super::*;

pub(super) async fn run() {
    let harness = Harness::start("p3-services-product").await;
    let storage = harness.storage.clone();
    let artifacts = harness.artifacts.clone();
    let transport = harness.transport.clone();
    let supervisor = harness.supervisor.clone();
    let version_pins = harness.version_pins.clone();
    let service_invocations = harness.service_invocations.clone();

    let account = storage.identity().default_account_id;
    let repository = WorkerRepository::new(storage.db());
    let (target, _) = repository
        .create_worker(
            account,
            "service-target",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let (asset_only, _) = repository
        .create_worker(account, "asset-target", RequestId::generate(), 2, 1_000_000)
        .unwrap();
    let (caller, _) = repository
        .create_worker(
            account,
            "service-caller",
            RequestId::generate(),
            3,
            1_000_000,
        )
        .unwrap();
    let (object_target, _) = repository
        .create_worker(
            account,
            "object-service-target",
            RequestId::generate(),
            4,
            1_000_000,
        )
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let socket_namespace = websocket_handoff::create_namespace(&harness, account, target.id);

    let target_v1 = deploy(
        &controller,
        worker_request(
            account,
            target.id,
            "target-v1",
            &target_source("v1"),
            WorkerRequestOptions {
                assets: Some(single_asset(&artifacts, "/asset.txt", b"asset-v1").await),
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("target-v1"))]),
                bindings: BTreeMap::from([(
                    "SOCKETS".to_owned(),
                    VersionBindingInput {
                        kind: BindingKind::DoNamespace,
                        id: socket_namespace,
                        permissions: CanonicalPermissions::default(),
                        config: CanonicalBindingConfig::default(),
                    },
                )]),
                services: BTreeMap::new(),
                promote: true,
                now_ms: 10,
            },
        ),
    )
    .await;
    let asset_version = deploy(
        &controller,
        assets_request(
            account,
            asset_only.id,
            "asset-only",
            single_asset(&artifacts, "/only.txt", b"only-asset").await,
            11,
        ),
    )
    .await;
    let object_version = deploy(
        &controller,
        worker_request(
            account,
            object_target.id,
            "object-target-v1",
            r#"export default {
  fetch(request, env) {
    return new Response(`object:${env.OWNER}:${new URL(request.url).hostname}`);
  }
};"#,
            WorkerRequestOptions {
                assets: None,
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("object-v1"))]),
                bindings: BTreeMap::new(),
                services: BTreeMap::new(),
                promote: true,
                now_ms: 12,
            },
        ),
    )
    .await;
    let services = BTreeMap::from([
        (
            "TARGET".to_owned(),
            VersionServiceInput {
                target_worker_id: target.id,
                entrypoint: None,
                props: Some(serde_json::json!({
                    "constructor": {"enabled": true},
                    "nested": [1, {"__proto__": "ordinary JSON data"}],
                    "region": "earth",
                })),
            },
        ),
        (
            "NAMED".to_owned(),
            VersionServiceInput {
                target_worker_id: target.id,
                entrypoint: Some("NamedApi".to_owned()),
                props: None,
            },
        ),
        (
            "ASSET_ONLY".to_owned(),
            VersionServiceInput {
                target_worker_id: asset_only.id,
                entrypoint: None,
                props: None,
            },
        ),
        (
            "OBJECT".to_owned(),
            VersionServiceInput {
                target_worker_id: object_target.id,
                entrypoint: None,
                props: None,
            },
        ),
        (
            "SELF".to_owned(),
            VersionServiceInput {
                target_worker_id: caller.id,
                entrypoint: None,
                props: None,
            },
        ),
    ]);
    let caller_version = deploy(
        &controller,
        worker_request(
            account,
            caller.id,
            "caller-v1",
            CALLER_SOURCE,
            WorkerRequestOptions {
                assets: None,
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("caller"))]),
                bindings: BTreeMap::new(),
                services,
                promote: true,
                now_ms: 13,
            },
        ),
    )
    .await;

    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/request-body",
        "streamed request",
    )
    .await;
    websocket_handoff::verify(
        &transport,
        account,
        caller.id,
        &caller_version,
        target_v1.id,
        &version_pins,
        &service_invocations,
    )
    .await;

    let first_asset = dispatch(&transport, account, caller.id, &caller_version, "/asset").await;
    let first_asset_status = first_asset.status();
    let first_asset_body = body(first_asset).await;
    assert_eq!(
        first_asset_status,
        StatusCode::OK,
        "first Service call failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&first_asset_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(first_asset_body.as_ref(), b"asset-v1");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let connect = dispatch(&transport, account, caller.id, &caller_version, "/connect").await;
    let connect_status = connect.status();
    let connect_body = body(connect).await;
    assert_eq!(
        connect_status,
        StatusCode::OK,
        "Service connect failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&connect_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(connect_body.as_ref(), b"7,8,9");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let connect_ipv6 = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/connect-ipv6",
    )
    .await;
    let connect_ipv6_status = connect_ipv6.status();
    let connect_ipv6_body = body(connect_ipv6).await;
    assert_eq!(
        connect_ipv6_status,
        StatusCode::OK,
        "IPv6 Service connect failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&connect_ipv6_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(connect_ipv6_body.as_ref(), b"10,11,12");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/object-fetch",
        "object:object-v1:object.example",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, object_version.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset-only",
        "only-asset",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/target-fetch",
        "fetch-v1:preserved.example:/worker",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/named-fetch",
        "named-fetch-v1:named.example",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/default-rpc",
    )
    .await;
    assert_eq!(identity.status(), StatusCode::OK);
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v1","owner":"target-v1"})
    );
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let props = dispatch(&transport, account, caller.id, &caller_version, "/props").await;
    assert_eq!(props.status(), StatusCode::OK);
    let props: serde_json::Value = serde_json::from_slice(&body(props).await).unwrap();
    assert_eq!(
        props,
        serde_json::json!({
            "constructor": {"enabled": true},
            "nested": [1, {"__proto__": "ordinary JSON data"}],
            "region": "earth",
        })
    );
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/named-rpc",
        "42",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset-only-rpc",
        "SERVICE_ENTRYPOINT_NOT_FOUND",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/background",
        "background-v1",
    )
    .await;
    assert_eq!(version_pins.count(target_v1.id), 1);
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/failure",
        "business-failure-v1",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;
    let capability = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/capability",
    )
    .await;
    let capability: serde_json::Value = serde_json::from_slice(&body(capability).await).unwrap();
    assert_eq!(
        capability,
        serde_json::json!({
            "first":"v1:cap:one",
            "callback":"callback:ok",
            "second":"label:v1:cap:nested",
        }),
    );
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;

    let target_v2 = deploy(
        &controller,
        worker_request(
            account,
            target.id,
            "target-v2",
            &target_source("v2"),
            WorkerRequestOptions {
                assets: Some(single_asset(&artifacts, "/asset.txt", b"asset-v2").await),
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("target-v2"))]),
                bindings: BTreeMap::from([(
                    "SOCKETS".to_owned(),
                    VersionBindingInput {
                        kind: BindingKind::DoNamespace,
                        id: socket_namespace,
                        permissions: CanonicalPermissions::default(),
                        config: CanonicalBindingConfig::default(),
                    },
                )]),
                services: BTreeMap::new(),
                promote: false,
                now_ms: 14,
            },
        ),
    )
    .await;
    let held = dispatch(&transport, account, caller.id, &caller_version, "/hold").await;
    assert_eq!(held.status(), StatusCode::OK);
    let mut held_body = held.into_body().into_data_stream();
    let ready = held_body.next().await.unwrap().unwrap();
    assert_eq!(ready.as_ref(), b"ready\n");
    let held_counts = service_invocations.counts();
    assert_eq!((held_counts.0, held_counts.2), (1, 1));
    assert!(held_counts.1 <= 1);
    assert_eq!(version_pins.count(target_v1.id), 1);
    assert_eq!(version_pins.count(caller_version.id), 2);
    repository
        .promote(
            account,
            target.id,
            target_v2.id,
            Some(target_v1.id),
            RequestId::generate(),
            15,
        )
        .unwrap();
    let mut held_tail = Vec::new();
    while let Some(chunk) = held_body.next().await {
        held_tail.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(held_tail, b"v1:held:later");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset",
        "asset-v2",
    )
    .await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/default-rpc",
    )
    .await;
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v2","owner":"target-v2"})
    );
    assert_eq!(version_pins.count(target_v1.id), 0);
    wait_pin_count(&version_pins, &service_invocations, target_v2.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/limit",
        "SERVICE_LIMIT_EXCEEDED",
    )
    .await;
    wait_service_counts(&service_invocations, (0, 0, 0)).await;
    assert!(version_pins.count(asset_version.id) <= 1);

    harness.stop().await;
}
