//! Large concurrent uploads through real HTTP, stock workerd, R2 and D1.

use super::*;
use futures::StreamExt as _;
use http_body_util::BodyExt as _;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use open_compute_storage::{R2MultipartRepository, R2MultipartState};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const TOTAL: usize = 241_910_375;
const PART: usize = 8 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_large_upload_keeps_runtime_responsive() {
    let config = R2Config {
        max_object_bytes: 256 * 1024 * 1024,
        max_staging_bytes: 64 * 1024 * 1024,
        max_concurrent_uploads: 4,
        ..R2Config::default()
    };
    // Explicit local-provider qualification only; the canonical Gate does not pass this env.
    let endpoint = std::env::var("OPEN_COMPUTE_TEST_R2_S3_ENDPOINT").ok();
    let gate = support::start(config.clone(), endpoint.as_deref()).await;
    let account = gate.storage.identity().default_account_id;
    let bucket = create_bucket(&gate.storage, &gate.objects, &config, account).await;
    let db = match ResourceController::new(
        &gate.storage,
        gate.pins.clone(),
        D1ResourceDriver::new(
            &gate.storage,
            open_compute_core::D1Config::default().database_quota_bytes,
        ),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::D1Database,
        name: "upload-index".to_owned(),
        idempotency_key: "upload-index".to_owned(),
        driver_schema_version: open_compute_storage::D1_DATABASE_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms: 20,
    })
    .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected resource replay"),
    };
    let (worker, _) = WorkerRepository::new(gate.storage.db())
        .create_worker(
            account,
            "large-upload",
            RequestId::generate(),
            21,
            1_000_000,
        )
        .unwrap();
    let versions = VersionController::new(
        &gate.storage,
        gate.artifacts.clone(),
        Arc::new(gate.transport.clone()),
        BundleLimits::default(),
    );
    let mut input = request(account, worker.id, bucket, "upload-version", SOURCE, 22);
    input.bindings.insert(
        "DB".to_owned(),
        VersionBindingInput {
            kind: BindingKind::D1Database,
            id: db,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    let version = deploy(&versions, input, &gate.supervisor).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let transport = gate.transport.clone();
    let router = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let transport = transport.clone();
        let version = version.clone();
        async move {
            transport
                .dispatch(
                    DispatchTarget {
                        account_id: account,
                        worker_id: worker.id,
                        version_id: version.id,
                        worker_code_sha256: hex::encode(version.worker_code_sha256),
                        entrypoint: None,
                        route_generation: 1,
                        request_id: RequestId::generate(),
                    },
                    request,
                )
                .await
                .unwrap()
        }
    });
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let client = Client::builder(TokioExecutor::new()).build_http::<Body>();
    let created =
        checked_json(send(&client, "POST", format!("{base}/create"), Body::empty()).await).await;
    let upload_id = created["uploadId"].as_str().unwrap().to_owned();
    let repo = R2MultipartRepository::new(gate.storage.db());
    let started = Instant::now();
    let uploads = futures::stream::iter(0..TOTAL.div_ceil(PART))
        .map(|index| {
            let client = client.clone();
            let url = format!("{base}/part?uploadId={upload_id}&number={}", index + 1);
            async move {
                let bytes = tokio::task::spawn_blocking(move || part_bytes(index))
                    .await
                    .unwrap();
                let started = Instant::now();
                let part = checked_json(send(&client, "PUT", url, Body::from(bytes)).await).await;
                (part, started.elapsed())
            }
        })
        .buffer_unordered(4)
        .collect::<Vec<_>>();
    tokio::pin!(uploads);
    let mut probes = 0;
    let mut max_probe = Duration::ZERO;
    let results = loop {
        tokio::select! {
            result = &mut uploads => break result,
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                let probe_started = Instant::now();
                let response = send(&client, "GET", format!("{base}/ping"), Body::empty()).await;
                assert_eq!(response.status(), 200);
                assert_eq!(response.into_body().collect().await.unwrap().to_bytes(), "pong");
                max_probe = max_probe.max(probe_started.elapsed());
                assert!(max_probe < Duration::from_secs(2), "lightweight request stalled: {max_probe:?}");
                probes += 1;
            }
        }
    };
    let upload_elapsed = started.elapsed();
    assert!(probes > 0, "no concurrent responsiveness probe executed");
    let max_part = results.iter().map(|(_, elapsed)| *elapsed).max().unwrap();
    let mut parts: Vec<_> = results.into_iter().map(|(part, _)| part).collect();
    parts.sort_by_key(|part| part["partNumber"].as_i64().unwrap());
    let persisted = repo.list_parts(&upload_id).unwrap();
    assert_eq!(persisted.len(), TOTAL.div_ceil(PART));
    for (stored, returned) in persisted.iter().zip(&parts) {
        assert_eq!(stored.part_number, returned["partNumber"]);
        assert_eq!(stored.etag, returned["etag"]);
    }
    assert_eq!(
        persisted.iter().map(|part| part.size).sum::<u64>(),
        TOTAL as u64
    );
    assert_eq!(
        repo.get(account, bucket, &upload_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Open
    );
    let before = send(&client, "GET", format!("{base}/get"), Body::empty()).await;
    assert_eq!(
        before.status(),
        404,
        "incomplete object must not be visible"
    );
    let indexed =
        checked_json(send(&client, "GET", format!("{base}/state"), Body::empty()).await).await;
    assert_eq!(indexed["count"], parts.len());
    assert_eq!(indexed["bytes"], TOTAL);
    let completed = checked_json(
        send(
            &client,
            "POST",
            format!("{base}/complete?uploadId={upload_id}"),
            Body::from(serde_json::to_vec(&parts).unwrap()),
        )
        .await,
    )
    .await;
    assert_eq!(completed["size"], TOTAL);
    assert_eq!(
        repo.get(account, bucket, &upload_id)
            .unwrap()
            .unwrap()
            .state,
        R2MultipartState::Completed
    );
    let response = send(&client, "GET", format!("{base}/get"), Body::empty()).await;
    assert_eq!(response.status(), 200);
    let mut stream = response.into_body().into_data_stream();
    let mut actual = Sha256::new();
    let mut length = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        length += chunk.len();
        actual.update(&chunk);
    }
    let expected = tokio::task::spawn_blocking(|| {
        let mut digest = Sha256::new();
        for index in 0..TOTAL.div_ceil(PART) {
            digest.update(part_bytes(index));
        }
        digest.finalize()
    })
    .await
    .unwrap();
    assert_eq!(length, TOTAL);
    assert_eq!(
        actual.finalize(),
        expected,
        "readback must match every uploaded byte"
    );
    assert!(
        std::fs::read_dir(gate.storage.data_dir().root().join("r2-staging"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(gate.pins.count(bucket), 0);
    let provider_parts = gate
        .mock
        .recorded()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.query.contains("partNumber="))
        .count();
    if endpoint.is_none() {
        assert_eq!(provider_parts, parts.len(), "unexpected provider retries");
    }
    println!(
        "large upload profile={} bytes={TOTAL} concurrency=4 upload_ms={} max_part_ms={} probes={probes} max_probe_ms={} sha256={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        upload_elapsed.as_millis(),
        max_part.as_millis(),
        max_probe.as_millis(),
        hex::encode(expected)
    );
    let _ = stop.send(());
    server.await.unwrap();
    gate.supervisor.shutdown().await;
    assert_eq!(gate.supervisor.owner_registry_len(), 0);
    let _ = gate.shutdown_tx.send(true);
    gate.source_task.await.unwrap().unwrap();
    gate.binding_task.await.unwrap().unwrap();
}

fn part_bytes(index: usize) -> Vec<u8> {
    let length = (TOTAL - index * PART).min(PART);
    (0..length)
        .map(|offset| ((offset % 251 + index * 17) % 256) as u8)
        .collect()
}

async fn send(
    client: &Client<HttpConnector, Body>,
    method: &str,
    url: String,
    body: Body,
) -> axum::http::Response<hyper::body::Incoming> {
    tokio::time::timeout(
        Duration::from_secs(35),
        client.request(
            Request::builder()
                .method(method)
                .uri(url)
                .body(body)
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn checked_json(response: axum::http::Response<hyper::body::Incoming>) -> Value {
    let status = response.status();
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, 200, "{text}");
    serde_json::from_str(&text).unwrap()
}

const SOURCE: &str = r#"export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/ping") return new Response("pong");
    if (url.pathname === "/create") {
      await env.DB.exec("CREATE TABLE parts (part INTEGER PRIMARY KEY, size INTEGER NOT NULL)");
      return Response.json(await env.BUCKET.createMultipartUpload("large.bin"));
    }
    if (url.pathname === "/part") {
      const bytes = await request.arrayBuffer();
      const number = Number(url.searchParams.get("number"));
      const upload = env.BUCKET.resumeMultipartUpload("large.bin", url.searchParams.get("uploadId"));
      const part = await upload.uploadPart(number, bytes);
      await env.DB.prepare("INSERT INTO parts VALUES (?, ?)").bind(number, bytes.byteLength).run();
      return Response.json(part);
    }
    if (url.pathname === "/state") {
      return Response.json(await env.DB.prepare("SELECT COUNT(*) AS count, SUM(size) AS bytes FROM parts").first());
    }
    if (url.pathname === "/complete") {
      const upload = env.BUCKET.resumeMultipartUpload("large.bin", url.searchParams.get("uploadId"));
      return Response.json(await upload.complete(await request.json()));
    }
    if (url.pathname === "/get") {
      const object = await env.BUCKET.get("large.bin");
      return object ? new Response(object.body) : new Response("missing", { status: 404 });
    }
    return new Response("missing", { status: 404 });
  }
};"#;
