//! Public HTTP API cases sharing the real P0.2 runtime.

use super::*;

pub(super) async fn api_matrix(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    account: open_compute_core::AccountId,
) {
    let health = HealthCoordinator::new();
    for component in [
        ComponentName::Process,
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::S3,
        ComponentName::Cache,
        ComponentName::Runtime,
    ] {
        health
            .set_component(
                component,
                ComponentState::Healthy,
                Some(ReadinessReason::Ready),
            )
            .unwrap();
    }
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "gate", "gate").unwrap());
    let state = HttpState::new(
        health,
        metrics,
        false,
        &ServerConfig::default(),
        Arc::new(|| None),
    )
    .unwrap()
    .with_worker_api(WorkerApiState::new(
        storage.clone(),
        artifacts,
        transport,
        DeploymentPins::new(),
        BundleLimits::default(),
        Duration::from_secs(5),
    ));
    let app = merged_router(state);
    worker_toolchain::exercise(app.clone(), storage.clone(), account).await;
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-worker-create")
                .body(Body::from(r#"{"name":"api-gate"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), 201);
    let create_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(create.into_body(), 64 * 1024).await.unwrap()).unwrap();
    let worker_id = create_json["worker"]["id"].as_str().unwrap();

    let replay_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-worker-create")
                .body(Body::from(r#"{"name":"api-gate"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay_create.status(), 201);
    let list_workers = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_workers.status(), 200);
    let get_worker = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers/{worker_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_worker.status(), 200);
    let missing_key = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"missing-key"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_key.status(), 400);

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: br#"import { WorkerEntrypoint } from "cloudflare:workers";
export class Named extends WorkerEntrypoint {
  async fetch() { return new Response("api:named"); }
}
export default { fetch(request, env) { return new Response('api:' + env.MODE); } };"#
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let metadata = serde_json::json!({
        "mainModule": "index.js",
        "compatibilityDate": "2026-08-22",
        "compatibilityFlags": [],
        "vars": {"MODE": "real"},
        "secrets": {},
        "limits": {"profile": "default"},
        "promote": true
    })
    .to_string();
    let bundle_chunks = bundle
        .into_bytes()
        .chunks(7)
        .map(|chunk| Ok::<Bytes, Infallible>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let deployment = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("idempotency-key", "api-deploy-create")
                .header("x-open-compute-deployment-metadata", metadata)
                .body(Body::from_stream(futures::stream::iter(bundle_chunks)))
                .unwrap(),
        )
        .await
        .unwrap();
    let deployment_status = deployment.status();
    let deployment_body = to_bytes(deployment.into_body(), 128 * 1024).await.unwrap();
    assert_eq!(
        deployment_status,
        201,
        "deployment response={}",
        String::from_utf8_lossy(&deployment_body)
    );
    let deployment_json: serde_json::Value = serde_json::from_slice(&deployment_body).unwrap();
    let deployment_id = deployment_json["deployment"]["id"].as_str().unwrap();
    let get_deployment = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments/{deployment_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_deployment.status(), 200);

    let named_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-route-named")
                .body(Body::from(
                    r#"{"hostname":"named.example.test","pathPrefix":"/named","entrypoint":"Named"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let named_route_status = named_route.status();
    let named_route_body = to_bytes(named_route.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(
        named_route_status,
        201,
        "named route response={}",
        String::from_utf8_lossy(&named_route_body)
    );
    let named_route_json: serde_json::Value = serde_json::from_slice(&named_route_body).unwrap();
    let named_route_id = named_route_json["route"]["id"].as_str().unwrap();
    let missing_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-route-missing")
                .body(Body::from(
                    r#"{"hostname":"named.example.test","pathPrefix":"/missing","entrypoint":"Missing"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_route.status(), 404);

    let named_public = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/named/hello")
                .header(header::HOST, "named.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(named_public.status(), 200);
    assert_eq!(
        to_bytes(named_public.into_body(), 1024).await.unwrap(),
        "api:named"
    );

    let list_routes = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/accounts/{account}/workers/{worker_id}/routes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_routes.status(), 200);
    let delete_named_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/routes/{named_route_id}"
                ))
                .header("idempotency-key", "api-route-delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_named_route.status(), 202);

    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/__workers/{account}/api-gate/hello"))
                .header(header::HOST, "public.example.test")
                .header("x-open-compute-deployment-id", "forged")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), 200);
    assert_eq!(
        to_bytes(public.into_body(), 1024).await.unwrap(),
        "api:real"
    );

    let disposable_bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('disposable'); } };".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let disposable_metadata = serde_json::json!({
        "mainModule": "index.js",
        "compatibilityDate": "2026-08-22",
        "compatibilityFlags": [],
        "vars": {},
        "secrets": {},
        "limits": {"profile": "default"},
        "promote": false
    })
    .to_string();
    let disposable = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header("idempotency-key", "api-deploy-disposable")
                .header("x-open-compute-deployment-metadata", disposable_metadata)
                .body(Body::from(disposable_bundle.into_bytes()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disposable.status(), 201);
    let disposable_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(disposable.into_body(), 128 * 1024).await.unwrap())
            .unwrap();
    let disposable_id = disposable_json["deployment"]["id"].as_str().unwrap();
    let promoted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/promotions"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-promote-disposable")
                .body(Body::from(
                    serde_json::json!({
                        "targetDeploymentId": disposable_id,
                        "expectedActiveDeploymentId": deployment_id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(promoted.status(), 200);
    let rolled_back = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/rollbacks"
                ))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-rollback-original")
                .body(Body::from(
                    serde_json::json!({
                        "targetDeploymentId": deployment_id,
                        "expectedActiveDeploymentId": disposable_id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rolled_back.status(), 200);
    let referenced_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments/{disposable_id}"
                ))
                .header("idempotency-key", "api-delete-referenced")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(referenced_delete.status(), 409);
    let _ = WorkerRepository::new(storage.db())
        .prune_expired_idempotency(i64::MAX, 1_000)
        .unwrap();
    let delete_request = || {
        Request::builder()
            .method("DELETE")
            .uri(format!(
                "/v1/accounts/{account}/workers/{worker_id}/deployments/{disposable_id}"
            ))
            .header("idempotency-key", "api-delete-complete")
            .body(Body::empty())
            .unwrap()
    };
    let deleted = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(deleted.status(), 202);
    let deleted_body = to_bytes(deleted.into_body(), 64 * 1024).await.unwrap();
    let replay = app.clone().oneshot(delete_request()).await.unwrap();
    assert_eq!(replay.status(), 202);
    assert_eq!(
        to_bytes(replay.into_body(), 64 * 1024).await.unwrap(),
        deleted_body
    );

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker_id}/deployments"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), 200);

    let disposable_worker = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{account}/workers"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "api-disposable-worker")
                .body(Body::from(r#"{"name":"api-disposable"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disposable_worker.status(), 201);
    let disposable_worker_json: serde_json::Value = serde_json::from_slice(
        &to_bytes(disposable_worker.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let disposable_worker_id = disposable_worker_json["worker"]["id"].as_str().unwrap();
    let deleted_worker = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{disposable_worker_id}"
                ))
                .header("idempotency-key", "api-disposable-worker-delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_worker.status(), 202);
}
