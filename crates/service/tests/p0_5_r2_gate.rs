//! Real pinned-workerd P0.5 R2 facade, lifecycle, and restart Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{PlatformConfig, RuntimeConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, R2Config, Redactor,
    RequestId, ResourceId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::{
    R2BindingService, SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend,
};
use open_compute_storage::{
    PlatformStorage, R2_SCHEMA_VERSION, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, VersionRecord, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, R2ResourceDriver, ResourcePins, RuntimeSource, RuntimeValidator,
    VersionBindingInput, VersionController,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_5_real_r2_facade_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let pins = ResourcePins::new();
    let r2_config = R2Config {
        max_object_bytes: 8 * 1024 * 1024,
        max_staging_bytes: 16 * 1024 * 1024,
        operation_timeout_ms: 5_000,
        ..R2Config::default()
    };
    let r2_service = Arc::new(
        R2BindingService::new(
            storage.clone(),
            pins.clone(),
            objects.clone(),
            r2_config.clone(),
        )
        .unwrap(),
    );
    let (shutdown_tx, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let binding_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                binding_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                Some(r2_service),
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.5-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
        .with_max_request_body(32 * 1024 * 1024);
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-5-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let resource = create_bucket(&storage, &objects, &r2_config, account).await;
    let repository = WorkerRepository::new(storage.db());
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());

    let (object_worker, _) = repository
        .create_worker(account, "r2-object", RequestId::generate(), 20, 1_000_000)
        .unwrap();
    let object = deploy(
        &versions,
        request(
            account,
            object_worker.id,
            resource,
            "object-v1",
            object_source(),
            21,
        ),
        &supervisor,
    )
    .await;
    let cold = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/matrix",
        "",
    )
    .await;
    assert_eq!(cold.status, 200, "{}", cold.body);
    assert_eq!(cold.loader_outcome, Some(LoaderOutcome::Cold));
    let matrix: serde_json::Value = serde_json::from_str(&cold.body).unwrap();
    assert_eq!(matrix["localFacade"], true);
    assert_eq!(matrix["rawHidden"], true);
    assert_eq!(
        matrix["envKeys"],
        serde_json::json!(["BUCKET", "BUCKET_ALIAS"])
    );
    assert_eq!(matrix["httpMetadata"], true);
    assert_eq!(matrix["headCustom"], "world");
    assert_eq!(matrix["firstBodyUsed"], false);
    assert_eq!(matrix["secondBodyUsed"], true);
    assert_eq!(matrix["secondConsumeRejected"], true);
    assert_eq!(matrix["body"], "hello");
    assert_eq!(matrix["blobType"], "text/plain");
    assert_eq!(matrix["range"], "ell");
    assert_eq!(matrix["rangeSize"], 5);
    assert_eq!(matrix["conditionHasBody"], false);
    assert_eq!(matrix["pageSeparated"], true);
    assert_eq!(matrix["typedArray"], serde_json::json!([1, 2]));
    assert_eq!(matrix["streamJson"]["ok"], true);
    assert_eq!(matrix["aliasVisible"], true);
    assert_eq!(matrix["checksumMd5"], "5d41402abc4b2a76b9719d911017c592");
    assert_eq!(matrix["versionOk"], true);
    assert_eq!(matrix["storageClass"], "InfrequentAccess");
    assert_eq!(matrix["ssecGetDenied"], true);
    assert_eq!(matrix["ssecBody"], "secret");
    assert!(!matrix["ssecMd5"].as_str().unwrap().is_empty());
    assert_eq!(matrix["onlyIfSkipped"], true);
    assert_eq!(matrix["multipartKey"], "mpu.txt");
    assert_eq!(matrix["startAfterOmitsHello"], true);

    let warm = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/head",
        "",
    )
    .await;
    assert_eq!((warm.status, warm.body.as_str()), (200, "hello"));
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    let cancelled = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/fake-cancel",
        "",
    )
    .await;
    assert_eq!(
        (cancelled.status, cancelled.body.as_str()),
        (200, "cancelled")
    );
    assert_eq!(pins.count(resource), 0);

    let named = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        Some("Named"),
        "/shape",
        "",
    )
    .await;
    assert_eq!(
        (named.status, named.body.as_str()),
        (200, "named:true:true")
    );

    for (name, source, expected, now) in [
        ("r2-function", function_source(), "function:true", 30_i64),
        ("r2-class", class_source(), "class:true:true", 40_i64),
    ] {
        let (worker, _) = repository
            .create_worker(account, name, RequestId::generate(), now, 1_000_000)
            .unwrap();
        let version = deploy(
            &versions,
            request(account, worker.id, resource, name, source, now + 1),
            &supervisor,
        )
        .await;
        let response = dispatch(&transport, account, worker.id, &version, None, "/shape", "").await;
        assert_eq!((response.status, response.body.as_str()), (200, expected));
    }

    let object_v2 = deploy(
        &versions,
        request(
            account,
            object_worker.id,
            resource,
            "object-v2",
            object_source(),
            50,
        ),
        &supervisor,
    )
    .await;
    repository
        .promote(
            account,
            object_worker.id,
            object.id,
            Some(object_v2.id),
            RequestId::generate(),
            51,
        )
        .unwrap();
    let rolled_back = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/head",
        "",
    )
    .await;
    assert_eq!(
        (rolled_back.status, rolled_back.body.as_str()),
        (200, "hello")
    );

    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let restarted = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/head",
        "",
    )
    .await;
    assert_eq!((restarted.status, restarted.body.as_str()), (200, "hello"));

    let deleted = dispatch(
        &transport,
        account,
        object_worker.id,
        &object,
        None,
        "/cleanup",
        "",
    )
    .await;
    assert_eq!((deleted.status, deleted.body.as_str()), (200, "clean"));
    assert_eq!(pins.count(resource), 0);
    let staging = storage.data_dir().root().join("r2-staging");
    assert!(std::fs::read_dir(staging).unwrap().next().is_none());

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert_eq!(pins.count(resource), 0);
    println!("P0.5 stock-workerd facade/CRUD/list/promotion/restart matrix PASS");
}

async fn create_bucket(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    config: &R2Config,
    account: AccountId,
) -> ResourceId {
    let fingerprint = storage.crypto().fingerprint_request(b"p0-5-r2-bucket");
    let resource_id = ResourceId::generate();
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::R2Bucket,
                name: "gate-bucket",
                idempotency_key: "p0-5-r2-bucket",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("unexpected resource reservation")
    };
    R2ResourceDriver::new(storage, objects.clone(), config.clone())
        .create(&resource)
        .await
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource_id, 11)
        .unwrap();
    resource_id
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
    supervisor: &WorkerdSupervisor,
) -> VersionRecord {
    match controller
        .create_version(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "version failed: {error:?}; runtime={:?}; diagnostics={:?}",
                supervisor.snapshot(),
                supervisor.last_diagnostics()
            )
        }) {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

fn request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    resource: ResourceId,
    key: &str,
    source: &str,
    now_ms: i64,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    bindings.insert(
        "BUCKET".to_owned(),
        VersionBindingInput {
            kind: BindingKind::R2Bucket,
            id: resource,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    bindings.insert(
        "BUCKET_ALIAS".to_owned(),
        VersionBindingInput {
            kind: BindingKind::R2Bucket,
            id: resource,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn object_source() -> &'static str {
    r#"import { WorkerEntrypoint } from "cloudflare:workers";
import { R2Bucket as ImportableR2Bucket } from "./__open_compute__/r2/facade.js";

function wrapped(bucket) {
  return bucket instanceof ImportableR2Bucket
    && typeof bucket.put === "function"
    && typeof bucket.fetch === "undefined";
}

export class Named extends WorkerEntrypoint {
  constructor(ctx, env) {
    super(ctx, env);
    this.constructorSawWrapped = wrapped(env.BUCKET);
  }
  async fetch() {
    return new Response(`named:${this.constructorSawWrapped}:${wrapped(this.env.BUCKET)}`);
  }
}

export default {
  async fetch(request, env) {
    const mark = async (name, promise) => {
      try { return await promise; } catch (error) { throw new Error(`${name}:${error}`); }
    };
    const path = new URL(request.url).pathname;
    if (path === "/head") return new Response(await (await env.BUCKET.get("hello.txt")).text());
    if (path === "/fake-cancel") {
      let cancelled = false;
      const unexpected = () => { throw new Error("unexpected R2 transport call"); };
      const bucket = new ImportableR2Bucket({
        head: unexpected, put: unexpected, delete: unexpected, list: unexpected,
        createMultipartUpload: unexpected, uploadPart: unexpected, completeMultipartUpload: unexpected, abortMultipartUpload: unexpected,
        async get() {
          return {
            meta: { key: "fake", version: "00000000-0000-7000-8000-000000000001", size: 1,
              etag: "0".repeat(32), httpEtag: `"${"0".repeat(32)}"`,
              uploaded: 0, httpMetadata: {}, customMetadata: {}, checksums: {}, storageClass: "Standard" },
            body: new ReadableStream({
              start(controller) { controller.enqueue(new Uint8Array([1])); },
              cancel() { cancelled = true; },
            }),
          };
        }
      });
      const reader = (await bucket.get("fake")).body.getReader();
      await reader.read();
      await reader.cancel("tenant cancelled");
      return new Response(cancelled ? "cancelled" : "not-cancelled");
    }
    if (path === "/cleanup") {
      await env.BUCKET.delete(["hello.txt", "typed.bin", "stream.json", "ia.bin", "ssec.bin", "mpu.txt"]);
      return new Response("clean");
    }
    if (path !== "/matrix") return new Response("missing", { status: 404 });
    let phase = "start";
    try {
    const localFacade = wrapped(env.BUCKET);
    const rawHidden = !Reflect.ownKeys(env.BUCKET).some((key) => String(key).includes("raw"));
    const envKeys = Object.keys(env).sort();
    phase = "put-hello";
    await mark("put-hello", env.BUCKET.put("hello.txt", "hello", {
      httpMetadata: { contentType: "text/plain", cacheControl: "max-age=60" },
      customMetadata: { greeting: "world" },
      md5: "5d41402abc4b2a76b9719d911017c592",
    }));
    const view = new Uint8Array([9, 1, 2, 9]).subarray(1, 3);
    phase = "put-typed";
    await mark("put-typed", env.BUCKET.put("typed.bin", view));
    phase = "put-stream";
    await mark("put-stream", env.BUCKET.put("stream.json", new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('{"ok":'));
        controller.enqueue(new TextEncoder().encode("true}"));
        controller.close();
      }
    }), { httpMetadata: new Headers({ "content-type": "application/json" }) }));
    phase = "head";
    const head = await mark("head", env.BUCKET.head("hello.txt"));
    const headers = new Headers();
    const returned = head.writeHttpMetadata(headers);
    const httpMetadata = returned === undefined
      && headers.get("content-type") === "text/plain"
      && headers.get("cache-control") === "max-age=60";
    phase = "get-first";
    const first = await mark("get-first", env.BUCKET.get("hello.txt"));
    const firstBodyUsed = first.bodyUsed;
    const body = await first.text();
    const secondBodyUsed = first.bodyUsed;
    let secondConsumeRejected = false;
    try { await first.bytes(); } catch { secondConsumeRejected = true; }
    phase = "blob";
    const blobType = (await (await env.BUCKET.get("hello.txt")).blob()).type;
    phase = "range";
    const ranged = await env.BUCKET.get("hello.txt", { range: { offset: 1, length: 3 } });
    const rangeSize = ranged.size;
    const range = await ranged.text();
    phase = "condition";
    const condition = await env.BUCKET.get("hello.txt", {
      onlyIf: { etagMatches: "does-not-match" },
    });
    phase = "list-1";
    const page1 = await env.BUCKET.list({ limit: 1, include: ["httpMetadata", "customMetadata"] });
    phase = "list-2";
    const page2 = await env.BUCKET.list({ limit: 1, cursor: page1.cursor, include: ["httpMetadata", "customMetadata"] });
    phase = "typed-get";
    const typedArray = Array.from(new Uint8Array(await (await env.BUCKET.get("typed.bin")).arrayBuffer()));
    phase = "stream-get";
    const streamJson = await (await env.BUCKET.get("stream.json")).json();
    const aliasVisible = (await env.BUCKET_ALIAS.head("hello.txt")).size === 5
      && wrapped(env.BUCKET_ALIAS);
    phase = "checksums";
    const checksumJson = first.checksums.toJSON();
    const versionOk = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(head.version);
    phase = "storage-class";
    const ia = await mark("storage-class", env.BUCKET.put("ia.bin", "ia", { storageClass: "InfrequentAccess" }));
    phase = "ssec";
    const ssecKey = "00".repeat(32);
    await mark("ssec-put", env.BUCKET.put("ssec.bin", "secret", { ssecKey }));
    const ssecHead = await mark("ssec-head", env.BUCKET.head("ssec.bin"));
    let ssecGetDenied = false;
    try { await env.BUCKET.get("ssec.bin"); } catch { ssecGetDenied = true; }
    const ssecBody = await (await env.BUCKET.get("ssec.bin", { ssecKey })).text();
    phase = "only-if";
    const skipped = await env.BUCKET.put("hello.txt", "nope", { onlyIf: { etagMatches: "missing" } });
    phase = "multipart";
    const created = await env.BUCKET.createMultipartUpload("mpu.txt");
    const resumed = env.BUCKET.resumeMultipartUpload(created.key, created.uploadId);
    const uploaded = await resumed.uploadPart(1, "multipart-body");
    const completed = await resumed.complete([uploaded]);
    phase = "start-after";
    const after = await env.BUCKET.list({ startAfter: "hello.txt", limit: 1000 });
    return Response.json({
      localFacade, rawHidden, envKeys, httpMetadata,
      headCustom: head.customMetadata.greeting,
      firstBodyUsed, secondBodyUsed, secondConsumeRejected, body, blobType,
      range, rangeSize, conditionHasBody: "body" in condition,
      pageSeparated: page1.truncated && page1.objects[0].key !== page2.objects[0].key,
      typedArray, streamJson, aliasVisible,
      checksumMd5: checksumJson.md5, versionOk, storageClass: ia.storageClass,
      ssecGetDenied, ssecBody, ssecMd5: ssecHead.ssecKeyMd5, onlyIfSkipped: skipped === null,
      multipartKey: completed.key, startAfterOmitsHello: after.objects.every((item) => item.key !== "hello.txt"),
    });
    } catch (error) {
      return new Response(`${phase}:${error && error.stack ? error.stack : error}`, { status: 598 });
    }
  }
};"#
}

fn function_source() -> &'static str {
    r#"export default async function(request, env) {
  const found = await env.BUCKET.head("hello.txt");
  return new Response(`function:${found !== null && typeof env.BUCKET.fetch === "undefined"}`);
}"#
}

fn class_source() -> &'static str {
    r#"import { WorkerEntrypoint } from "cloudflare:workers";
export default class extends WorkerEntrypoint {
  constructor(ctx, env) {
    super(ctx, env);
    this.constructorSawWrapped = typeof env.BUCKET.put === "function" && typeof env.BUCKET.fetch === "undefined";
  }
  async fetch() {
    const methodSawWrapped = typeof this.env.BUCKET.get === "function" && typeof this.env.BUCKET.fetch === "undefined";
    return new Response(`class:${this.constructorSawWrapped}:${methodSawWrapped}`);
  }
}"#
}

struct DispatchResponse {
    status: u16,
    body: String,
    loader_outcome: Option<LoaderOutcome>,
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &VersionRecord,
    entrypoint: Option<&str>,
    path: &str,
    body: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "r2.test")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: entrypoint.map(str::to_owned),
                route_generation: 1,
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "runtime did not become ready: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, old_pid: i32, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn stores(mock: &MockS3) -> (ArtifactStore, R2ObjectStore) {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
r2_prefix = "tenant/r2/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    let client = S3ArtifactClient::connect(&config, &credentials, 64 * 1024 * 1024).unwrap();
    (
        ArtifactStore::new(client.clone()),
        R2ObjectStore::new(client),
    )
}

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::default().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    config
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
