//! One cohesive genuine Python runtime/FFI case on the formally pinned daemon path.

use super::ai_search;
use super::ai_search_provider::Provider;
use super::artifacts;
use super::assertions;
use super::assets;
use super::cache;
use super::images;
use super::python_support::capture::{Bindings, Capture};
use super::python_support::fixture::Fixture;
use super::python_support::{PYTHON_SECRETS, platform_process};
use super::search;
use super::{PEER, PYTHON};
use super::{dynamic, http_clients};
use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};
use std::fs;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p21_python_runtime_ffi_stdlib_wait_until_network_restart_rollback() {
    let mut capture = Capture::load(
        "test/fixtures/python-runtime",
        "test/applications/python-runtime/src",
        &[
            "cache_cases.py",
            "image_cases.py",
            "ai_cases.py",
            "vector_cases.py",
            "artifact_cases.py",
            "search_cases.py",
            "http_client_cases.py",
        ],
    )
    .await;
    let provider = Provider::start().await;
    let mut fixture = Fixture::new(Some(provider.config())).await;
    let dynamic_version = dynamic::deploy(&fixture).await;
    dynamic::baseline(&fixture, "fresh-process-before-python-preparation").await;
    let http = http_clients::Server::start().await;
    search::create_index(&fixture).await;
    artifacts::create_namespaces(&fixture).await;
    ai_search::create_resources(&fixture).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let outbound = format!("http://{address}/");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        let router=axum::Router::new()
            .route("/",axum::routing::post(|headers:HeaderMap,body:Bytes| async move {
                (StatusCode::ACCEPTED,Json(json!({"body":String::from_utf8(body.to_vec()).unwrap(),"caller":headers["x-caller"].to_str().unwrap()})))
            }))
            .route("/tcp",axum::routing::get(|| async {"python-runtime-tcp"}));
        let mut connections = tokio::task::JoinSet::new();
        let mut stopped = stopped;
        loop {
            let (stream, _) = tokio::select! {
                accepted = listener.accept() => accepted.unwrap(),
                _ = &mut stopped => break,
            };
            let service = hyper_util::service::TowerToHyperService::new(router.clone());
            connections.spawn(async move {
                // The TCP clients half-close their request; keep the response writable until complete.
                let _ = hyper::server::conn::http1::Builder::new()
                    .half_close(true)
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
        while let Some(connection) = connections.join_next().await {
            connection.unwrap();
        }
    });
    let kv = fixture
        .api(
            "/storage/kv/namespaces",
            "POST",
            Some(json!({"title":"python-runtime-kv"})),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let db = fixture
        .api(
            "/d1/database",
            "POST",
            Some(json!({"name":"python-runtime-db"})),
        )
        .await["uuid"]
        .as_str()
        .unwrap()
        .to_owned();
    let bucket = "python-runtime-bucket";
    fixture
        .api("/r2/buckets", "POST", Some(json!({"name":bucket})))
        .await;
    let inputs = Bindings {
        kv: &kv,
        d1: &db,
        r2: bucket,
        outbound: &outbound,
        revision: "first",
        secret: PYTHON_SECRETS[0],
    };
    capture.metadata["assets"]["jwt"] =
        json!(assets::upload(&fixture, PYTHON, assets::FIRST).await);
    let first = python_version(&fixture, &capture, &inputs).await;
    let first_record = fixture.record(PYTHON, &first);
    fixture.promote(PYTHON, &first).await;
    let peer_assets = json!({"jwt":assets::upload(&fixture, PEER, assets::FIRST).await,"config":capture.metadata["assets"]["config"]});
    let peer = fixture
        .upload_javascript(
            PEER,
            include_str!("peer.js"),
            &[
                json!({"name":"KV","type":"kv_namespace","namespace_id":kv}),
                json!({"name":"DB","type":"d1","id":db}),
                json!({"name":"BUCKET","type":"r2_bucket","bucket_name":bucket}),
                json!({"name":"OUTBOUND_URL","type":"plain_text","text":outbound}),
                json!({"name":"ASSETS","type":"assets"}),
                json!({"name":"IMAGES","type":"images"}),
                json!({"name":"AI","type":"ai"}),
                json!({"name":"VECTORS","type":"vectorize","index_name":search::INDEX}),
                json!({"name":"ARTIFACTS","type":"artifacts","namespace":artifacts::NAMESPACE}),
                json!({"name":"ISOLATED_ARTIFACTS","type":"artifacts","namespace":artifacts::ISOLATED}),
                json!({"name":"SEARCH","type":"ai_search_namespace","namespace":"default"}),
                json!({"name":"DIRECT_SEARCH","type":"ai_search","instance_name":ai_search::INSTANCE}),
                json!({"name":"ISOLATED_SEARCH","type":"ai_search_namespace","namespace":ai_search::ISOLATED}),
            ],
            None,
            Some(peer_assets),
        )
        .await;
    fixture.promote(PEER, &peer).await;
    assertions::ffi(&fixture).await;
    assertions::stdlib(&fixture).await;
    assertions::outbound(&fixture).await;
    http_clients::matrix(&fixture, &http).await;
    dynamic::baseline(&fixture, "after-ordinary-python-preparation").await;
    capture.metadata["limits"] = json!({"subrequests": 2});
    capture.metadata["assets"]["jwt"] =
        json!(assets::upload(&fixture, PYTHON, assets::FIRST).await);
    let limited = python_version(&fixture, &capture, &inputs).await;
    capture.metadata.as_object_mut().unwrap().remove("limits");
    fixture.promote(PYTHON, &limited).await;
    http_clients::budget(&fixture, &http).await;
    fixture.promote(PYTHON, &first).await;
    let mut search_state = ai_search::initialize(&fixture).await;
    let search_first = ai_search::retained(&fixture, &search_state).await;
    cache::matrix(&fixture).await;
    assets::parity(&fixture, assets::FIRST).await;
    images::parity(&fixture).await;
    search::initialize(&fixture).await;
    search::retained(&fixture, "first").await;
    artifacts::initialize(&fixture).await;
    let artifacts_first = artifacts::retained(&fixture, false).await;
    for (script, path, value) in [
        (PYTHON, "/write", "first/sdk"),
        (PYTHON, "/write?caller=ffi", "first/ffi"),
        (PEER, "/write", "javascript"),
    ] {
        assert_eq!(fixture.invoke(script, path).await, json!({"written":value}));
        assertions::read(&fixture, value).await;
    }
    assertions::background(&fixture, "first").await;
    assertions::boundaries(&fixture).await;
    assertions::read(&fixture, "javascript").await;
    assert_eq!(
        fixture.invoke(PYTHON, "/fs-write").await,
        json!({"value":"first"})
    );

    fixture.process.stop().await;
    fixture.ciphertext(&first_record).await;
    fixture.mock.clear_recorded();
    fixture.process.restart(&fixture.config, &fixture.log);
    platform_process::ready(&fixture.client, fixture.admin, &mut fixture.process).await;
    assert_eq!(
        fixture.invoke(PYTHON, "/fs-read").await,
        json!({"value":null})
    );
    assert_eq!(fixture.invoke(PEER, "/background-read").await, "first");
    assertions::no_prepare_upload(&fixture);
    dynamic::baseline(&fixture, "fresh-process-after-restart").await;
    assert_eq!(
        fixture.active(dynamic::SCRIPT),
        uuid::Uuid::parse_str(&dynamic_version).unwrap().to_string()
    );
    assert_eq!(fixture.record(PYTHON, &first), first_record);

    capture.metadata["assets"]["jwt"] =
        json!(assets::upload(&fixture, PYTHON, assets::SECOND).await);
    let second = python_version(
        &fixture,
        &capture,
        &Bindings {
            revision: "second",
            secret: PYTHON_SECRETS[1],
            ..inputs
        },
    )
    .await;
    let second_record = fixture.record(PYTHON, &second);
    assert_ne!(
        first_record.prepared_identity_sha256,
        second_record.prepared_identity_sha256
    );
    assert_ne!(first_record.artifact_sha256, second_record.artifact_sha256);
    assert_eq!(fixture.active(PYTHON), normalized(&first));
    fixture.promote(PYTHON, &second).await;
    cache::new_version(&fixture).await;
    assets::parity(&fixture, assets::SECOND).await;
    search::update(&fixture).await;
    artifacts::update(&fixture).await;
    search::retained(&fixture, "second").await;
    let artifacts_second = artifacts::retained(&fixture, true).await;
    assert_eq!(artifacts_second["inspected"], artifacts_first["inspected"]);
    ai_search::update(&fixture, &mut search_state).await;
    let search_second = ai_search::retained(&fixture, &search_state).await;
    assert_eq!(search_second["info"], search_first["info"]);
    assert_eq!(search_second["job"], search_first["job"]);
    assert_eq!(
        fixture.invoke(PYTHON, "/fs-read").await,
        json!({"value":null})
    );
    assert_eq!(
        fixture.invoke(PYTHON, "/write?caller=ffi").await,
        json!({"written":"second/ffi"})
    );
    assertions::read(&fixture, "second/ffi").await;
    assertions::background(&fixture, "second").await;
    fixture.promote(PYTHON, &first).await;
    fixture.mock.clear_recorded();
    fixture.restart().await;
    assert_eq!(
        fixture.invoke(PYTHON, "/").await,
        json!({"revision":"first"})
    );
    assert_eq!(
        fixture.invoke(PYTHON, "/fs-read").await,
        json!({"value":null})
    );
    assert_eq!(fixture.invoke(PEER, "/background-read").await, "second");
    assertions::no_prepare_upload(&fixture);
    assert_eq!(fixture.record(PYTHON, &first), first_record);
    assert_eq!(fixture.record(PYTHON, &second), second_record);

    fixture.process.stop().await;
    provider.finish().await;
    http.finish().await;
    for lease in [
        "child.lease",
        "python-prepare.lease",
        "python-compile.lease",
    ] {
        assert!(!fixture.data.join("runtime").join(lease).exists());
    }
    for address in [fixture.public, fixture.admin] {
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    }
    let log = fs::read(&fixture.log).unwrap();
    for secret in PYTHON_SECRETS {
        assert!(
            !log.windows(secret.len())
                .any(|part| part == secret.as_bytes())
        );
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    println!(
        "python-runtime-evidence: {}",
        json!({
            "uploadSha256":capture.sha256,"firstVersion":first,"secondVersion":second,"javascriptPeerVersion":peer,
            "firstPreparedIdentitySha256":hex::encode(first_record.prepared_identity_sha256),
            "secondPreparedIdentitySha256":hex::encode(second_record.prepared_identity_sha256),
            "sdkAndFfiSharedBindings":["KV","D1","R2"],"ephemeralFilesystemNotRestored":true,
            "nativeCacheDefaultAndNamed":true,"cacheWorkerAndNamespaceIsolationAndSharedVersionState":true,
            "assetsSdkFfiJavascriptParityAndVersionRouting":true,
            "imagesSdkFfiJavascriptRasterAndStreamParity":true,
            "aiMarkdownAndVectorizeSdkFfiJavascriptParity":true,
            "artifactsExtensionNativeFfiJavascriptAuthorityParity":true,
            "aiSearchExtensionNativeFfiJavascriptAndProviderStreamParity":true,
            "httpAndTcpHostNetwork":true,"secretsAbsentFromLogs":true,
        })
    );
}

fn normalized(version: &str) -> String {
    uuid::Uuid::parse_str(version).unwrap().to_string()
}

async fn python_version(fixture: &Fixture, capture: &Capture, bindings: &Bindings<'_>) -> String {
    let mut values = bindings.metadata();
    values.push(json!({"name":"ASSETS","type":"assets"}));
    values.push(json!({"name":"IMAGES","type":"images"}));
    values.push(json!({"name":"AI","type":"ai"}));
    values.push(json!({"name":"VECTORS","type":"vectorize","index_name":search::INDEX}));
    values.push(json!({"name":"ARTIFACTS","type":"artifacts","namespace":artifacts::NAMESPACE}));
    values.push(json!({"name":"SEARCH","type":"ai_search_namespace","namespace":"default"}));
    values.push(
        json!({"name":"DIRECT_SEARCH","type":"ai_search","instance_name":ai_search::INSTANCE}),
    );
    values.push(json!({"name":"ISOLATED_SEARCH","type":"ai_search_namespace","namespace":ai_search::ISOLATED}));
    let (status, _, bytes, version) = fixture
        .upload_version(
            PYTHON,
            &Capture::content_type(),
            capture.render(&values, None),
        )
        .await;
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        status, 200,
        "Python runtime upload/prepare failed: {}",
        response["errors"]
    );
    assert_eq!(response["success"], true);
    version.unwrap().to_string()
}
