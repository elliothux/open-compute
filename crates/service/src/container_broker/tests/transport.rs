use super::*;
use crate::container_broker::{QualifiedContainerProvider, run_broker};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::Response,
    routing::any,
};
use sha2::Sha256;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, oneshot};

#[path = "directory_engine.rs"]
mod directory_engine;
#[path = "recovery.rs"]
mod recovery;
#[path = "sidecar_engine.rs"]
mod sidecar_engine;
#[path = "snapshot_engine.rs"]
mod snapshot_engine;
pub(super) use recovery::helper_input;
use sidecar_engine::sidecar;

#[derive(Default)]
struct MockEngine {
    containers: Mutex<BTreeMap<String, Value>>,
    requests: Mutex<Vec<String>>,
    published_port: AtomicU16,
    credentials: Mutex<Option<crate::container_broker::BrokerControlCredentials>>,
    reverse: Mutex<Option<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>>,
    archive_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    archive_started: tokio::sync::Notify,
    archive_failure: AtomicBool,
    delete_failure: AtomicBool,
    delete_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    delete_started: tokio::sync::Notify,
    next_container: AtomicU64,
    inspect_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    inspect_started: tokio::sync::Notify,
    wait_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    wait_body_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    wait_started: tokio::sync::Notify,
    stop_signals: Mutex<Vec<(String, String)>>,
    stop_seconds: Mutex<Vec<u64>>,
    imports: Mutex<Vec<Vec<u8>>>,
    directory_uploads: Mutex<Vec<(String, Vec<u8>)>>,
    directory_volumes: Mutex<BTreeMap<String, Value>>,
    directory_helper_exit: AtomicU64,
    directory_upload_failure: AtomicBool,
    import_failure: AtomicBool,
    image_overrides: Mutex<BTreeMap<String, (u16, Value)>>,
    exec_requests: Mutex<Vec<Value>>,
    executions: Mutex<BTreeMap<String, Value>>,
    next_exec: AtomicU64,
    exec_inspect_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    exec_inspect_started: tokio::sync::Notify,
    snapshot_failure: Mutex<Option<&'static str>>,
    snapshot_archive: Mutex<Option<Vec<u8>>>,
    snapshot_gate: Mutex<Option<(&'static str, Arc<tokio::sync::Notify>)>>,
    snapshot_started: tokio::sync::Notify,
    snapshot_changed_start: AtomicBool,
    snapshot_qualification_failure: Mutex<Option<&'static str>>,
    marker_archive_error: Mutex<Option<(u16, Vec<u8>)>>,
    marker_archive_error_once: AtomicBool,
    marker_archive_exited: AtomicBool,
    marker_archive_calls: AtomicU64,
    helper_volume_override: Mutex<Option<(u16, Value)>>,
    helper_archive_override: Mutex<Option<(u16, Vec<u8>)>>,
    helper_image_archive_override: Mutex<Option<(u16, Vec<u8>)>>,
    sidecar_profile_override: Mutex<Option<(u16, Vec<u8>)>>,
    sidecar_auth_delay: Mutex<Option<std::time::Duration>>,
    sidecar_auth_rejections: AtomicU64,
    sidecar_callback_auth_status: AtomicU16,
    sidecar_profile_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

fn image_config() -> Value {
    json!({"Volumes":{}, "Env":["KEY=default", "IMAGE_DEFAULT=value"],
        "Cmd":["image-command"], "Entrypoint":["image-entrypoint"], "User":"1000", "WorkingDir":"/app", "Labels":{"image.label":"default"}, "StopSignal":"SIGKILL"})
}

async fn exec_environment_checks_validated_request_before_engine(
    client: &reqwest::Client,
    state: &MockEngine,
    lease: &BrokerLease,
) {
    let endpoint = format!("http://localhost/containers/{}/exec", lease.lease_key);
    for overrides in [
        None,
        Some(json!([])),
        Some(json!([
            "KEY=exec",
            "IMAGE_DEFAULT=exec",
            "PATH=/exec/bin",
            "HOME=/exec-home",
            "HOSTNAME=exec-host",
            "TERM=exec-term"
        ])),
    ] {
        let mut request = json!({"Cmd":["env"],"Tty":true,"AttachStdout":true});
        if let Some(overrides) = &overrides {
            request["Env"] = overrides.clone();
        }
        let response = client
            .post(&endpoint)
            .body(serde_json::to_vec(&request).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let requests = state.exec_requests.lock().await;
        let actual = requests.last().unwrap();
        assert_eq!(actual["Cmd"], request["Cmd"]);
        assert_eq!(actual["AttachStdout"], true);
        let environment = actual["Env"].as_array().unwrap();
        assert_eq!(
            &environment[..4],
            json!(["HOSTNAME", "IMAGE_DEFAULT", "KEY", "TERM"])
                .as_array()
                .unwrap()
        );
        assert_eq!(
            &environment[4..],
            overrides
                .as_ref()
                .map_or(&[][..], |value| value.as_array().unwrap().as_slice())
        );
    }
    let previous = state.exec_requests.lock().await.len();
    let response = client
        .post(&endpoint)
        .body(serde_json::to_vec(&json!({"Cmd":["env"],"Env":vec!["KEY=exec";513]})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(state.exec_requests.lock().await.len(), previous);
}

async fn engine(State(state): State<Arc<MockEngine>>, mut request: Request) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let query: BTreeMap<_, _> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let method = request.method().clone();
    state.requests.lock().await.push(format!("{method} {path}"));
    let upgraded = if path.starts_with("/exec/")
        && path.ends_with("/start")
        && request.headers().contains_key("Upgrade")
    {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };
    let bytes = to_bytes(request.into_body(), 16 << 20).await.unwrap();
    if path.starts_with("/exec/")
        && path.ends_with("/json")
        && let Some(gate) = state.exec_inspect_gate.lock().await.take()
    {
        state.exec_inspect_started.notify_one();
        gate.notified().await;
    }
    if let Some(response) = directory_engine::handle(&state, &method, &path, &bytes).await {
        return response;
    }
    if let Some(response) = snapshot_engine::handle(&state, &method, &path, &query).await {
        return response;
    }
    if let Some(response) = helper_input::handle(&state, &method, &path, &query).await {
        return response;
    }
    if path == "/images/load" {
        assert_eq!(method, Method::POST);
        assert_eq!(query.get("quiet").map(String::as_str), Some("true"));
        state.imports.lock().await.push(bytes.to_vec());
        let event = if state.import_failure.swap(false, Ordering::SeqCst) {
            json!({"error":"import rejected","errorDetail":{"message":"import rejected"}})
        } else {
            json!({"stream":format!("Loaded image ID: {}\n", lease().image)})
        };
        let mut response = serde_json::to_vec(&event).unwrap();
        response.push(b'\n');
        let split = response.len() / 2;
        let chunks = [response[..split].to_vec(), response[split..].to_vec()];
        return Response::builder()
            .header("Content-Type", "application/json")
            .body(Body::from_stream(futures::stream::iter(
                chunks.map(Ok::<_, std::convert::Infallible>),
            )))
            .unwrap();
    }
    if method == Method::PUT
        && path.ends_with("/archive")
        && (query
            .get("path")
            .is_some_and(|path| path.starts_with("/open-compute-directories/"))
            || (query.get("path").map(String::as_str) == Some("/")
                && state.containers.lock().await.values().any(|container| {
                    path.split('/').nth(2) == container["Id"].as_str()
                        && container
                            .pointer("/Config/Labels/dev.open-compute.container.role")
                            .and_then(Value::as_str)
                            == Some("directory-archive")
                })))
    {
        state
            .directory_uploads
            .lock()
            .await
            .push((query["path"].clone(), bytes.to_vec()));
        return Response::builder()
            .status(
                if state.directory_upload_failure.swap(false, Ordering::SeqCst)
                    || directory_engine::parent_link_escapes(&bytes)
                {
                    500
                } else {
                    200
                },
            )
            .body(Body::empty())
            .unwrap();
    }
    if let Some(upgraded) = upgraded {
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut socket = hyper_util::rt::TokioIo::new(upgraded.await.unwrap());
            let mut input = [0_u8; 5];
            socket.read_exact(&mut input).await.unwrap();
            assert_eq!(&input, b"hello");
            socket.write_all(&[1, 0, 0]).await.unwrap();
            socket
                .write_all(&[0, 0, 0, 0, 3, b'o', b'u', b't'])
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        return Response::builder()
            .status(101)
            .header("Connection", "Upgrade")
            .header("Upgrade", "tcp")
            .body(Body::empty())
            .unwrap();
    }

    if path.ends_with("/archive")
        && query.get("path").map(String::as_str) == Some("/run/open-compute")
    {
        use std::io::Read as _;
        let credentials = {
            let mut archive = tar::Archive::new(std::io::Cursor::new(&bytes));
            let mut entry = archive.entries().unwrap().next().unwrap().unwrap();
            assert_eq!(entry.header().mode().unwrap(), 0o600);
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            serde_json::from_slice(&data).unwrap()
        };
        let gate = state.archive_gate.lock().await.take();
        if let Some(gate) = gate {
            state.archive_started.notify_one();
            gate.notified().await;
        }
        *state.credentials.lock().await = Some(credentials);
        let status = if state.archive_failure.swap(false, Ordering::SeqCst) {
            500
        } else {
            200
        };
        return Response::builder()
            .status(status)
            .body(Body::empty())
            .unwrap();
    }
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if method == Method::DELETE {
        let gate = state.delete_gate.lock().await.take();
        if let Some(gate) = gate {
            state.delete_started.notify_one();
            gate.notified().await;
        }
    } else if path.ends_with("/wait") {
        let gate = state.wait_gate.lock().await.take();
        if let Some(gate) = gate {
            assert_eq!(query.get("condition").map(String::as_str), Some("removed"));
            state.wait_started.notify_one();
            gate.notified().await;
        }
        if let Some(gate) = state.wait_body_gate.lock().await.take() {
            let state = state.clone();
            return Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .body(Body::from_stream(futures::stream::once(async move {
                    state.wait_started.notify_one();
                    gate.notified().await;
                    Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"{\"StatusCode\":7}"))
                })))
                .unwrap();
        }
    } else if method == Method::GET && path.ends_with("-proxy/json") {
        let gate = state.inspect_gate.lock().await.take();
        if let Some(gate) = gate {
            state.inspect_started.notify_one();
            gate.notified().await;
        }
    }
    if path == "/_ping" {
        return Response::builder()
            .status(200)
            .body(Body::from("OK"))
            .unwrap();
    }
    let (status, value) = if path == "/info" {
        (
            200,
            json!({"ServerVersion":"29.4.0","Architecture":"amd64","OSType":"linux","Driver":"overlay2","DockerRootDir":"/docker","CgroupVersion":"2","SecurityOptions":["name=seccomp,profile=builtin"]}),
        )
    } else if path == "/version" {
        (200, json!({"Version":"29.4.0", "ApiVersion":"1.54"}))
    } else if path.starts_with("/images/") && path.ends_with("/json") {
        let id = path
            .strip_prefix("/images/")
            .unwrap()
            .strip_suffix("/json")
            .unwrap();
        state.image_overrides.lock().await.get(id).cloned().unwrap_or_else(|| (
            200,
            json!({"Id":id,"Size":73,"Os":"linux","Architecture":"amd64","Config":image_config(),"RootFS":{"Type":"layers","Layers":lease().diff_ids}}),
        ))
    } else if path == "/networks/bridge" {
        (200, json!({"IPAM":{"Config":[{"Subnet":"172.17.0.0/16"}]}}))
    } else if path == "/containers/json" {
        let containers = state.containers.lock().await;
        let entries: Vec<_> = containers.iter().filter(|(_, value)| value.pointer("/Config/Labels/dev.open-compute.container.lease").is_some())
            .map(|(name, value)| json!({"Names":[format!("/{name}")], "Labels":value["Config"]["Labels"]})).collect();
        (200, json!(entries))
    } else if path == "/containers/create" {
        let name = query.get("name").unwrap().clone();
        let mut containers = state.containers.lock().await;
        let mut host = body["HostConfig"].clone();
        if let Some(sidecar_name) = host
            .get("NetworkMode")
            .and_then(Value::as_str)
            .and_then(|mode| mode.strip_prefix("container:"))
        {
            let Some(sidecar) = containers.get(sidecar_name) else {
                return Response::builder().status(404).body(Body::empty()).unwrap();
            };
            host["NetworkMode"] = json!(format!("container:{}", sidecar["Id"].as_str().unwrap()));
        }
        match containers.entry(name) {
            Entry::Occupied(_) => (409, json!({"message":"raw-private-details"})),
            Entry::Vacant(slot) => {
                let mut effective = body.clone();
                let defaults = image_config();
                for field in ["User", "WorkingDir", "StopSignal"] {
                    if effective.get(field).is_none() {
                        effective[field] = defaults[field].clone();
                    }
                }
                // Moby only restores the image command when the requested entrypoint is empty.
                if effective
                    .get("Entrypoint")
                    .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
                {
                    if effective
                        .get("Cmd")
                        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
                    {
                        effective["Cmd"] = defaults["Cmd"].clone();
                    }
                    if effective.get("Entrypoint").is_none() {
                        effective["Entrypoint"] = defaults["Entrypoint"].clone();
                    }
                }
                let mut environment = effective
                    .get("Env")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for variable in defaults["Env"].as_array().unwrap() {
                    let name = variable.as_str().unwrap().split('=').next().unwrap();
                    if !environment
                        .iter()
                        .any(|value| value.as_str().unwrap().split('=').next() == Some(name))
                    {
                        environment.push(variable.clone());
                    }
                }
                effective["Env"] = json!(environment);
                let mut labels = defaults["Labels"].as_object().unwrap().clone();
                if let Some(overrides) = effective.get("Labels").and_then(Value::as_object) {
                    labels.extend(overrides.clone());
                }
                effective["Labels"] = json!(labels);
                let sequence = state.next_container.fetch_add(1, Ordering::Relaxed);
                let id = hex::encode(Sha256::digest(format!("{}:{sequence}", slot.key())));
                let value = json!({"Id":id,"Name":format!("/{}",slot.key()),"Image":body["Image"],"Config":effective,
                "GraphDriver":{"Name":"overlay2","Data":{"UpperDir":format!("/docker/overlay2/{id}/diff")}},
                "Mounts":helper_input::mounts_from_request(&host),
                "HostConfig":host,"State":{"Running":false,"Paused":false,"Status":"created","ExitCode":0,"Pid":0,
                "StartedAt":if effective.pointer("/Labels/dev.open-compute.container.role").and_then(Value::as_str)==Some("directory-archive") {"0001-01-01T00:00:00Z"} else {"2026-10-10T00:00:00Z"}},
                "NetworkSettings":{"Ports":{"39001/tcp":[{"HostIp":"0.0.0.0","HostPort":state.published_port.load(Ordering::Relaxed).to_string()}]}}});
                slot.insert(value);
                (201, json!({"Id":id,"Warnings":[]}))
            }
        }
    } else if path.starts_with("/containers/") {
        let parts: Vec<_> = path.split('/').collect();
        let mut containers = state.containers.lock().await;
        let name = containers
            .iter()
            .find(|(_, value)| value.get("Id").and_then(Value::as_str) == Some(parts[2]))
            .map_or_else(|| parts[2].to_owned(), |(name, _)| name.clone());
        let name = name.as_str();
        if method == Method::DELETE {
            if state.delete_failure.swap(false, Ordering::SeqCst) {
                (500, Value::Null)
            } else {
                let existed = containers.remove(name).is_some();
                (if existed { 204 } else { 404 }, Value::Null)
            }
        } else if parts.get(3) == Some(&"json") {
            match containers.get(name) {
                Some(value) => (200, value.clone()),
                None => (404, json!({"message":"raw-private-details"})),
            }
        } else if parts.get(3) == Some(&"wait") {
            (
                200,
                json!({"StatusCode":containers.get(name).and_then(|value| value.pointer("/State/ExitCode")).and_then(Value::as_u64).unwrap_or(0)}),
            )
        } else if parts.get(3) == Some(&"start") {
            match containers.get_mut(name) {
                Some(value) => {
                    directory_engine::start(
                        value,
                        state.directory_helper_exit.load(Ordering::SeqCst),
                    );
                    (204, Value::Null)
                }
                None => (404, Value::Null),
            }
        } else if parts.get(3) == Some(&"stop") {
            assert_eq!(query.get("signal").map(String::as_str), Some("SIGTERM"));
            let seconds: u64 = query["t"].parse().unwrap();
            assert!(seconds <= 900);
            state.stop_seconds.lock().await.push(seconds);
            let value = containers.get_mut(name).unwrap();
            state.stop_signals.lock().await.push((
                value["Config"]["StopSignal"].as_str().unwrap().into(),
                query["signal"].clone(),
            ));
            value["State"]["Running"] = false.into();
            (204, Value::Null)
        } else if parts.get(3) == Some(&"exec") {
            state
                .exec_requests
                .lock()
                .await
                .push(serde_json::from_slice(&bytes).unwrap());
            let sequence = state.next_exec.fetch_add(1, Ordering::Relaxed);
            let id = if sequence == 0 {
                "8".repeat(64)
            } else {
                hex::encode(Sha256::digest(format!("exec:{sequence}")))
            };
            let container = containers.get(name).unwrap()["Id"].clone();
            state.executions.lock().await.insert(
                id.clone(),
                json!({
                    "ID":id,"ContainerID":container,"Running":false,"ExitCode":0,"Pid":0
                }),
            );
            (201, json!({"Id":id}))
        } else {
            (204, Value::Null)
        }
    } else if path.starts_with("/exec/") {
        let id = path.split('/').nth(2).unwrap();
        match state.executions.lock().await.get(id) {
            Some(value) => (200, value.clone()),
            None => (404, json!({"message":"raw-private-details"})),
        }
    } else {
        (400, json!({"message":"raw-private-details"}))
    };
    Response::builder()
        .status(status)
        .body(Body::from(if value.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&value).unwrap()
        }))
        .unwrap()
}

struct TestListener(tokio::net::UnixListener);
impl axum::serve::Listener for TestListener {
    type Io = tokio::net::UnixStream;
    type Addr = tokio::net::unix::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        self.0.accept().await.unwrap()
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

#[tokio::test]
async fn container_broker_transport_registration_fencing_and_positive_termination() {
    let directory = tempfile::Builder::new()
        .prefix("oc-cbt-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory_path = std::fs::canonicalize(directory.path()).unwrap();
    let engine_path = directory_path.join("engine.sock");
    let state = Arc::new(MockEngine::default());
    let ca_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    state
        .published_port
        .store(ca_listener.local_addr().unwrap().port(), Ordering::Relaxed);
    let auth_state = state.clone();
    let ca_task = tokio::spawn(async move {
        axum::serve(
            ca_listener,
            Router::new().fallback(any(sidecar)).with_state(auth_state),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::UnixListener::bind(&engine_path).unwrap();
    let app = Router::new()
        .fallback(any(engine))
        .with_state(state.clone());
    let engine_task =
        tokio::spawn(async move { axum::serve(TestListener(listener), app).await.unwrap() });
    let mut config = config();
    config.socket = directory_path.join("broker.sock");
    config.engine_socket = engine_path;
    config.state_dir = directory_path.join("state");
    open_compute_storage::fs::ensure_dir_secure(&config.state_dir).unwrap();
    config.spool_dir = directory_path;
    let (stop, shutdown) = oneshot::channel();
    let broker_config = config.clone();
    let broker = tokio::spawn(async move {
        run_broker(broker_config, async {
            let _ = shutdown.await;
        })
        .await
    });
    let client = reqwest::Client::builder()
        .unix_socket(config.socket.clone())
        .no_proxy()
        .build()
        .unwrap();
    let mut reply = None;
    for _ in 0..100 {
        if let Ok(response) = client
            .get("http://localhost/open-compute/v1/contract")
            .send()
            .await
        {
            reply = Some(response);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let contract: ProviderContract =
        serde_json::from_slice(&reply.unwrap().bytes().await.unwrap()).unwrap();
    assert!(contract.qualified);
    let provider_config = open_compute_core::config::ContainersConfig {
        broker_socket: config.socket.clone(),
        provider_contract_sha256: contract.sha256().unwrap(),
        max_snapshots: 32,
        max_snapshot_bytes: 8 << 30,
        max_instances: 2,
        resources: resources(),
    };
    let provider = QualifiedContainerProvider::preflight(&provider_config)
        .await
        .unwrap();
    assert_eq!(provider.max_instances(), 2);
    assert_eq!(provider.resources(), resources());
    assert_eq!(provider.max_image_bytes(), 512 << 20);
    assert_eq!(provider.contract_sha256(), contract.sha256().unwrap());
    materialization_checks_staging_before_engine_import(&provider, &state, &config.spool_dir).await;
    image_proof_checks_current_engine_without_import(&provider, &provider_config, &state).await;
    let mut lease = lease();
    lease.provider_contract_sha256 = contract.sha256().unwrap();
    provider.register(&lease).await.unwrap();
    provider.register(&lease).await.unwrap();
    let mut policy_change = lease.clone();
    policy_change.max_instances = 1;
    provider.register(&policy_change).await.unwrap();
    let mut over_capacity = policy_change.clone();
    over_capacity.lease_key = "9".repeat(64);
    assert!(provider.register(&over_capacity).await.is_err());
    let mut changed_execution = policy_change.clone();
    changed_execution.startup.enable_internet = true;
    assert!(provider.register(&changed_execution).await.is_err());
    provider.register(&lease).await.unwrap();
    let first = provider
        .lease_credentials(&lease.lease_key, &"1".repeat(64))
        .await
        .unwrap();
    assert_eq!(
        first,
        provider
            .lease_credentials(&lease.lease_key, &"1".repeat(64))
            .await
            .unwrap()
    );
    assert!(!format!("{first:?}").contains(&first.control_token));
    let proxy = format!("{}-proxy", lease.lease_key);
    let missing = client
        .delete(format!("http://localhost/containers/{proxy}?force=true"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        client
            .post(format!(
                "http://localhost/containers/{proxy}/wait?condition=removed"
            ))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let observation = provider.observe(&lease.lease_key).await.unwrap();
    assert!(
        !observation.terminated
            && !observation.cleanup_complete
            && !observation.late_operations_fenced
    );
    assert_eq!(
        first,
        provider
            .lease_credentials(&lease.lease_key, &"1".repeat(64))
            .await
            .unwrap()
    );
    for sidecar in [true, false] {
        let name = format!("{}{}", lease.lease_key, if sidecar { "-proxy" } else { "" });
        let result = client
            .post(format!("http://localhost/containers/create?name={name}"))
            .body(serde_json::to_vec(&create(sidecar)).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), 201);
        if !sidecar {
            canonical_network_requires_the_verified_owned_proxy(&client, &provider, &state, &lease)
                .await;
        }
        let result = client
            .post(format!("http://localhost/containers/{name}/start"))
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), 204);
        if sidecar {
            let original_labels = state.containers.lock().await[&name]["Config"]["Labels"].clone();
            state.containers.lock().await.get_mut(&name).unwrap()["Config"]["Labels"] = json!({});
            let rejected = client
                .delete(format!("http://localhost/containers/{name}?force=true"))
                .send()
                .await
                .unwrap();
            assert_eq!(rejected.status(), 503);
            assert!(state.containers.lock().await.contains_key(&name));
            state.containers.lock().await.get_mut(&name).unwrap()["Config"]["Labels"] =
                original_labels;
            let id = state.containers.lock().await[&name]["Id"]
                .as_str()
                .unwrap()
                .to_owned();
            let wait_gate = Arc::new(tokio::sync::Notify::new());
            *state.wait_gate.lock().await = Some(wait_gate.clone());
            let waiting = client.clone();
            let waited_name = name.clone();
            let waiter = tokio::spawn(async move {
                waiting
                    .post(format!(
                        "http://localhost/containers/{waited_name}/wait?condition=removed"
                    ))
                    .send()
                    .await
                    .unwrap()
            });
            state.wait_started.notified().await;
            let gate = Arc::new(tokio::sync::Notify::new());
            *state.delete_gate.lock().await = Some(gate.clone());
            let inspect_gate = Arc::new(tokio::sync::Notify::new());
            *state.inspect_gate.lock().await = Some(inspect_gate.clone());
            let inspecting = client.clone();
            let inspected_name = name.clone();
            let inspection = tokio::spawn(async move {
                inspecting
                    .get(format!("http://localhost/containers/{inspected_name}/json"))
                    .send()
                    .await
                    .unwrap()
            });
            state.inspect_started.notified().await;
            let deleting = client.clone();
            let deleted_name = name.clone();
            let deletion = tokio::spawn(async move {
                deleting
                    .delete(format!(
                        "http://localhost/containers/{deleted_name}?force=true"
                    ))
                    .send()
                    .await
                    .unwrap()
            });
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(20),
                    state.delete_started.notified()
                )
                .await
                .is_err(),
                "proxy deletion overtook an in-flight inspection"
            );
            inspect_gate.notify_one();
            let inspected = inspection.await.unwrap();
            assert_eq!(inspected.status(), 200);
            let inspected: Value =
                serde_json::from_slice(&inspected.bytes().await.unwrap()).unwrap();
            let old_relay = super::super::relay::published_port(&inspected).unwrap();
            state.delete_started.notified().await;
            let creating = client.clone();
            let created_name = name.clone();
            let replacement = tokio::spawn(async move {
                creating
                    .post(format!(
                        "http://localhost/containers/create?name={created_name}"
                    ))
                    .body(serde_json::to_vec(&create(true)).unwrap())
                    .send()
                    .await
                    .unwrap()
            });
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(!replacement.is_finished());
            gate.notify_one();
            assert_eq!(deletion.await.unwrap().status(), 204);
            assert_eq!(replacement.await.unwrap().status(), 201);
            assert!(!waiter.is_finished());
            wait_gate.notify_one();
            assert_eq!(waiter.await.unwrap().status(), 200);
            assert!(
                state
                    .requests
                    .lock()
                    .await
                    .contains(&format!("POST /containers/{id}/wait"))
            );
            assert!(
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, old_relay))
                    .await
                    .is_err()
            );
            assert_ne!(
                state.containers.lock().await[&name]["Id"].as_str().unwrap(),
                id
            );
            assert!(
                state
                    .requests
                    .lock()
                    .await
                    .contains(&format!("DELETE /containers/{id}"))
            );
            assert_eq!(
                client
                    .post(format!("http://localhost/containers/{name}/start"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                204
            );
            assert_eq!(
                first,
                provider
                    .lease_credentials(&lease.lease_key, &"1".repeat(64))
                    .await
                    .unwrap()
            );
        }
    }
    let running_proxy_delete = client
        .delete(format!("http://localhost/containers/{proxy}?force=true"))
        .send()
        .await
        .unwrap();
    assert_eq!(running_proxy_delete.status(), 503);
    assert!(state.containers.lock().await.contains_key(&proxy));
    assert_eq!(
        client
            .post(format!(
                "http://localhost/containers/{}/wait",
                lease.lease_key
            ))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    state
        .containers
        .lock()
        .await
        .get_mut(&lease.lease_key)
        .unwrap()["State"]["Running"] = false.into();
    // Restart the stopped application in its existing namespace; Docker binds it to a proxy ID.
    for name in [&proxy, &lease.lease_key] {
        assert_eq!(
            client
                .post(format!("http://localhost/containers/{name}/start"))
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    // Sixteen mandatory idle callbacks cannot consume the eight native HTTP slots.
    let proxy_inspect = client
        .get(format!(
            "http://localhost/containers/{}-proxy/json",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(proxy_inspect.status(), 200);
    let proxy_inspect: Value =
        serde_json::from_slice(&proxy_inspect.bytes().await.unwrap()).unwrap();
    let relay_port = super::super::relay::published_port(&proxy_inspect).unwrap();
    let mut idle_callbacks = Vec::new();
    for _ in 0..16 {
        idle_callbacks.push(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, relay_port))
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let started = client
        .post(format!(
            "http://localhost/containers/{}/start",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 204);
    assert!(provider.observe(&lease.lease_key).await.unwrap().running);
    exec_environment_checks_validated_request_before_engine(&client, &state, &lease).await;
    let exec_capacity = client
        .post(format!(
            "http://localhost/containers/{}/exec",
            lease.lease_key
        ))
        .body(serde_json::to_vec(&json!({"Cmd":["echo","capacity"]})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(exec_capacity.status(), 201);
    provider.ensure_active().await.unwrap();
    let rotated = provider
        .lease_credentials(&lease.lease_key, &"2".repeat(64))
        .await
        .unwrap();
    assert_ne!(first, rotated);
    for mut callback in idle_callbacks {
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                callback.read(&mut [0_u8; 1])
            )
            .await
            .unwrap()
            .unwrap(),
            0
        );
    }

    assert_eq!(state.credentials.lock().await.as_ref(), Some(&rotated));
    assert!(
        provider
            .lease_credentials(&lease.lease_key, "invalid")
            .await
            .is_err()
    );
    let observation = provider.observe(&lease.lease_key).await.unwrap();
    assert!(observation.running);
    assert_ne!(observation.start_identity, lease.start_identity);
    assert_eq!(
        provider.sidecar_image(),
        contract.sidecar_image.as_deref().unwrap()
    );
    assert_eq!(provider.architecture(), "amd64");
    state
        .containers
        .lock()
        .await
        .get_mut(&lease.lease_key)
        .unwrap()["State"]["StartedAt"] = "different-process".into();
    assert!(provider.observe(&lease.lease_key).await.is_err());
    state
        .containers
        .lock()
        .await
        .get_mut(&lease.lease_key)
        .unwrap()["State"]["StartedAt"] = "2026-10-10T00:00:00Z".into();
    let original_environment =
        state.containers.lock().await[&lease.lease_key]["Config"]["Env"].clone();
    state
        .containers
        .lock()
        .await
        .get_mut(&lease.lease_key)
        .unwrap()["Config"]["Env"]
        .as_array_mut()
        .unwrap()
        .push(json!("UNADMITTED=extra"));
    assert!(provider.observe(&lease.lease_key).await.is_err());
    let tampered_start = client
        .post(format!(
            "http://localhost/containers/{}/start",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(tampered_start.status(), 503);
    state
        .containers
        .lock()
        .await
        .get_mut(&lease.lease_key)
        .unwrap()["Config"]["Env"] = original_environment;
    let original_start_identity = observation.start_identity;
    stop.send(()).unwrap();
    broker.await.unwrap().unwrap();
    assert!(!config.socket.exists());
    let (stop, shutdown) = oneshot::channel();
    let broker_config = config.clone();
    let broker = tokio::spawn(async move {
        run_broker(broker_config, async {
            let _ = shutdown.await;
        })
        .await
    });
    for _ in 0..100 {
        if config.socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let provider = QualifiedContainerProvider::preflight(&provider_config)
        .await
        .unwrap();
    assert_ne!(state.credentials.lock().await.as_ref(), Some(&rotated));
    let unknown_start = client
        .post(format!(
            "http://localhost/containers/{}/start",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_start.status(), 503);
    provider.register(&lease).await.unwrap();
    let resumed = provider
        .lease_credentials(&lease.lease_key, &"2".repeat(64))
        .await
        .unwrap();
    assert_ne!(resumed, rotated);
    assert_eq!(state.credentials.lock().await.as_ref(), Some(&resumed));
    assert_eq!(
        provider
            .observe(&lease.lease_key)
            .await
            .unwrap()
            .start_identity,
        original_start_identity
    );
    let wrong = client
        .post("http://localhost/images/create?fromImage=latest")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 503);
    assert!(!wrong.text().await.unwrap().contains("raw-private"));
    let exec = client
        .post(format!(
            "http://localhost/containers/{}/exec",
            lease.lease_key
        ))
        .body(serde_json::to_vec(&json!({"Cmd":["echo","hi"]})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(exec.status(), 201);
    let exec = client
        .post(format!("http://localhost/exec/{}/start", "8".repeat(64)))
        .body(serde_json::to_vec(&json!({"Detach":true,"Tty":false})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(exec.status(), 200);
    let upgraded = client
        .post(format!("http://localhost/exec/{}/start", "8".repeat(64)))
        .header("Connection", "Upgrade")
        .header("Upgrade", "tcp")
        .body(serde_json::to_vec(&json!({"Detach":false,"Tty":false})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), 101);
    let mut socket = upgraded.upgrade().await.unwrap();
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    socket.write_all(b"hello").await.unwrap();
    socket.shutdown().await.unwrap();
    let mut frames = Vec::new();
    socket.read_to_end(&mut frames).await.unwrap();
    assert_eq!(frames, [1, 0, 0, 0, 0, 0, 0, 3, b'o', b'u', b't']);
    drop(socket);
    // Cancellation after engine acceptance retains a durable fence across restart.
    let gate = Arc::new(tokio::sync::Notify::new());
    *state.archive_gate.lock().await = Some(gate.clone());
    let rotating_provider = provider.clone();
    let rotating_key = lease.lease_key.clone();
    let rotating = tokio::spawn(async move {
        rotating_provider
            .lease_credentials(&rotating_key, &"3".repeat(64))
            .await
    });
    state.archive_started.notified().await;
    let pending_path = config
        .state_dir
        .join(format!("pending-{}", lease.lease_key));
    assert_eq!(std::fs::read(&pending_path).unwrap(), b"v1\n");
    let later_provider = provider.clone();
    let later_key = lease.lease_key.clone();
    let later = tokio::spawn(async move {
        later_provider
            .lease_credentials(&later_key, &"4".repeat(64))
            .await
    });
    stop.send(()).unwrap();
    broker.await.unwrap().unwrap();
    assert!(rotating.await.unwrap().is_err());
    assert!(later.await.unwrap().is_err());
    assert!(provider.ensure_active().await.is_err());
    let (stop, shutdown) = oneshot::channel();
    let broker_config = config.clone();
    let broker = tokio::spawn(async move {
        run_broker(broker_config, async {
            let _ = shutdown.await;
        })
        .await
    });
    for _ in 0..100 {
        if config.socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let provider = QualifiedContainerProvider::preflight(&provider_config)
        .await
        .unwrap();
    provider.register(&lease).await.unwrap();
    assert!(
        provider
            .lease_credentials(&lease.lease_key, &"5".repeat(64))
            .await
            .is_err()
    );
    assert!(pending_path.exists());
    let fenced_inspect = client
        .get(format!(
            "http://localhost/containers/{}-proxy/json",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(fenced_inspect.status(), 503);
    gate.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // Late completion cannot make the adopted lease startable or rotate its credentials.
    assert!(
        provider
            .lease_credentials(&lease.lease_key, &"6".repeat(64))
            .await
            .is_err()
    );
    assert!(
        provider
            .terminate_gracefully(&lease.lease_key, 900_001)
            .await
            .is_err()
    );
    provider
        .terminate_gracefully(&lease.lease_key, 5)
        .await
        .unwrap();
    assert_eq!(
        *state.stop_signals.lock().await,
        vec![("SIGKILL".into(), "SIGTERM".into())]
    );
    provider.terminate(&lease.lease_key).await.unwrap();
    assert!(!pending_path.exists());
    let observation = provider.observe(&lease.lease_key).await.unwrap();
    assert!(!observation.running);
    assert!(
        observation.terminated
            && observation.cleanup_complete
            && observation.late_operations_fenced
    );
    assert!(provider.register(&lease).await.is_err());
    assert!(
        provider
            .lease_credentials(&lease.lease_key, &"3".repeat(64))
            .await
            .is_err()
    );
    let late = client
        .post(format!(
            "http://localhost/containers/{}/start",
            lease.lease_key
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(late.status(), 503);
    assert_eq!(state.containers.lock().await.len(), 0);
    // An engine error after a credential write is unknown, never permission to retry.
    let mut failed_lease = lease.clone();
    failed_lease.lease_key = "b".repeat(64);
    provider.register(&failed_lease).await.unwrap();
    provider
        .lease_credentials(&failed_lease.lease_key, &"7".repeat(64))
        .await
        .unwrap();
    let proxy = format!("{}-proxy", failed_lease.lease_key);
    assert_eq!(
        client
            .post(format!("http://localhost/containers/create?name={proxy}"))
            .body(serde_json::to_vec(&create(true)).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    state.archive_failure.store(true, Ordering::SeqCst);
    assert!(
        provider
            .lease_credentials(&failed_lease.lease_key, &"8".repeat(64))
            .await
            .is_err()
    );
    assert!(
        config
            .state_dir
            .join(format!("pending-{}", failed_lease.lease_key))
            .exists()
    );
    assert!(
        provider
            .lease_credentials(&failed_lease.lease_key, &"9".repeat(64))
            .await
            .is_err()
    );
    provider.terminate(&failed_lease.lease_key).await.unwrap();
    assert!(
        !config
            .state_dir
            .join(format!("pending-{}", failed_lease.lease_key))
            .exists()
    );
    assert_eq!(state.containers.lock().await.len(), 0);
    let mut delete_lease = lease.clone();
    delete_lease.lease_key = "c".repeat(64);
    provider.register(&delete_lease).await.unwrap();
    provider
        .lease_credentials(&delete_lease.lease_key, &"a".repeat(64))
        .await
        .unwrap();
    let proxy = format!("{}-proxy", delete_lease.lease_key);
    assert_eq!(
        client
            .post(format!("http://localhost/containers/create?name={proxy}"))
            .body(serde_json::to_vec(&create(true)).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    state.delete_failure.store(true, Ordering::SeqCst);
    assert_eq!(
        client
            .delete(format!("http://localhost/containers/{proxy}?force=true"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    let pending = config
        .state_dir
        .join(format!("pending-{}", delete_lease.lease_key));
    assert!(pending.exists());
    // Even positive proxy removal and a subsequent missing proxy cannot clear an earlier fence.
    for status in [204, 404] {
        assert_eq!(
            client
                .delete(format!("http://localhost/containers/{proxy}?force=true"))
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
        assert!(pending.exists());
        assert!(
            provider
                .lease_credentials(&delete_lease.lease_key, &"a".repeat(64))
                .await
                .is_err()
        );
        assert_eq!(
            client
                .post(format!("http://localhost/containers/create?name={proxy}"))
                .body(serde_json::to_vec(&create(true)).unwrap())
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
    }
    provider.terminate(&delete_lease.lease_key).await.unwrap();
    assert!(!pending.exists());
    stop.send(()).unwrap();
    broker.await.unwrap().unwrap();
    assert!(!config.socket.exists());
    engine_task.abort();
    ca_task.abort();
}

async fn canonical_network_requires_the_verified_owned_proxy(
    client: &reqwest::Client,
    provider: &QualifiedContainerProvider,
    state: &MockEngine,
    lease: &BrokerLease,
) {
    let name = &lease.lease_key;
    let proxy_name = format!("{name}-proxy");
    let original = state.containers.lock().await[name].clone();
    let proxy = state.containers.lock().await[&proxy_name].clone();
    assert_eq!(
        original["HostConfig"]["NetworkMode"],
        json!(format!("container:{}", proxy["Id"].as_str().unwrap()))
    );
    for network in [
        format!("container:{proxy_name}"),
        format!("container:{}", "f".repeat(64)),
    ] {
        state.containers.lock().await.get_mut(name).unwrap()["HostConfig"]["NetworkMode"] =
            json!(network);
        assert_eq!(
            client
                .get(format!("http://localhost/containers/{name}/json"))
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
        assert!(provider.observe(name).await.is_err());
    }
    state.containers.lock().await.insert(name.clone(), original);
    state.containers.lock().await.remove(&proxy_name);
    assert_eq!(
        client
            .post(format!("http://localhost/containers/{name}/start"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert!(provider.observe(name).await.is_err());
    state
        .containers
        .lock()
        .await
        .insert(proxy_name.clone(), proxy.clone());
    for (pointer, forged) in [
        (
            "/Config/Labels/dev.open-compute.container.lease",
            json!("f".repeat(64)),
        ),
        ("/HostConfig/Privileged", json!(true)),
        ("/Image", json!(format!("sha256:{}", "f".repeat(64)))),
    ] {
        let mut forged_proxy = proxy.clone();
        if pointer == "/HostConfig/Privileged" {
            forged_proxy["HostConfig"]["Privileged"] = forged;
        } else {
            *forged_proxy.pointer_mut(pointer).unwrap() = forged;
        }
        state
            .containers
            .lock()
            .await
            .insert(proxy_name.clone(), forged_proxy);
        assert_eq!(
            client
                .post(format!("http://localhost/containers/{name}/start"))
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
        assert!(provider.observe(name).await.is_err());
        state
            .containers
            .lock()
            .await
            .insert(proxy_name.clone(), proxy.clone());
    }
    assert_eq!(
        client
            .get(format!("http://localhost/containers/{name}/json"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(!provider.observe(name).await.unwrap().running);
    assert!(
        !state
            .requests
            .lock()
            .await
            .iter()
            .any(|request| request == &format!("POST /containers/{name}/start"))
    );
}

async fn image_proof_checks_current_engine_without_import(
    provider: &QualifiedContainerProvider,
    config: &open_compute_core::config::ContainersConfig,
    state: &MockEngine,
) {
    let mut lease = lease();
    lease.provider_contract_sha256 = provider.contract_sha256().to_owned();
    let image = json!({"Id":lease.image,"Size":73,"Os":"linux","Architecture":"amd64",
        "Config":{"Volumes":{}},"RootFS":{"Type":"layers","Layers":lease.diff_ids}});
    let imports = state.imports.lock().await.len();
    assert_eq!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .unwrap(),
        Some(73)
    );
    // Reconstructing the provider must re-inspect Engine state, not consult a host-memory hit.
    let restarted = QualifiedContainerProvider::preflight(config).await.unwrap();
    assert_eq!(
        restarted
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .unwrap(),
        Some(73)
    );
    // A derived image is checked by its own configuration and graph, never the base image.
    let derived_id = format!("sha256:{}", "6".repeat(64));
    let derived_layers = vec![format!("sha256:{}", "7".repeat(64))];
    let derived = json!({"Id":derived_id,"Size":41,"Os":"linux","Architecture":"amd64",
        "Config":{"Volumes":{}},"RootFS":{"Type":"layers","Layers":derived_layers}});
    state
        .image_overrides
        .lock()
        .await
        .insert(derived_id.clone(), (200, derived));
    assert_eq!(
        provider
            .image_size(&derived_id, &derived_layers)
            .await
            .unwrap(),
        Some(41)
    );
    assert_eq!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .unwrap(),
        Some(73)
    );
    assert!(
        provider
            .image_size(&derived_id, &lease.diff_ids)
            .await
            .is_err()
    );
    state.image_overrides.lock().await.remove(&derived_id);
    for pointer in [
        "/Id",
        "/Os",
        "/Architecture",
        "/RootFS/Type",
        "/RootFS/Layers/0",
    ] {
        let mut bad = image.clone();
        *bad.pointer_mut(pointer).unwrap() = "wrong".into();
        state
            .image_overrides
            .lock()
            .await
            .insert(lease.image.clone(), (200, bad));
        assert!(
            provider
                .image_size(&lease.image, &lease.diff_ids)
                .await
                .is_err()
        );
    }
    for size in [
        json!(-1),
        json!(0.5),
        json!("73"),
        json!(null),
        json!(true),
        json!(1e30),
    ] {
        let mut bad = image.clone();
        bad["Size"] = size;
        state
            .image_overrides
            .lock()
            .await
            .insert(lease.image.clone(), (200, bad));
        assert!(
            provider
                .image_size(&lease.image, &lease.diff_ids)
                .await
                .is_err()
        );
    }
    let mut missing = image.clone();
    missing.as_object_mut().unwrap().remove("Size");
    state
        .image_overrides
        .lock()
        .await
        .insert(lease.image.clone(), (200, missing));
    assert!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .is_err()
    );
    let mut zero = image.clone();
    zero["Size"] = json!(0);
    state
        .image_overrides
        .lock()
        .await
        .insert(lease.image.clone(), (200, zero));
    assert_eq!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .unwrap(),
        Some(0)
    );
    let mut bad = image;
    bad["Config"]["Volumes"] = json!({"/data":{}});
    state
        .image_overrides
        .lock()
        .await
        .insert(lease.image.clone(), (200, bad));
    assert!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .is_err()
    );
    for status in [404, 500] {
        state
            .image_overrides
            .lock()
            .await
            .insert(lease.image.clone(), (status, json!({})));
        let result = provider.image_size(&lease.image, &lease.diff_ids).await;
        if status == 404 {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(result.is_err());
        }
    }
    state.image_overrides.lock().await.remove(&lease.image);
    assert_eq!(
        provider
            .image_size(&lease.image, &lease.diff_ids)
            .await
            .unwrap(),
        Some(73)
    );
    // Deletion between a positive image check and execution admission stays unavailable.
    state
        .image_overrides
        .lock()
        .await
        .insert(lease.image.clone(), (404, json!({})));
    let before = state.containers.lock().await.len();
    assert!(provider.register(&lease).await.is_err());
    assert_eq!(state.containers.lock().await.len(), before);
    state.image_overrides.lock().await.remove(&lease.image);
    let requests = state.requests.lock().await.len();
    assert!(
        provider
            .image_size("mutable:tag", &lease.diff_ids)
            .await
            .is_err()
    );
    assert_eq!(state.requests.lock().await.len(), requests);
    assert_eq!(state.imports.lock().await.len(), imports);
}

async fn materialization_checks_staging_before_engine_import(
    provider: &QualifiedContainerProvider,
    state: &MockEngine,
    directory: &std::path::Path,
) {
    use std::io::{Seek as _, Write as _};

    let directory_entries = std::fs::read_dir(directory).unwrap().count();
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        (
            "manifest.json",
            br#"[{"Config":"config.json","RepoTags":[],"Layers":[]}]"#.as_slice(),
        ),
        (
            "config.json",
            br#"{"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[]},"config":{}}"#.as_slice(),
        ),
    ] {
        let mut header = tar::Header::new_ustar();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, bytes).unwrap();
    }
    let archive = builder.into_inner().unwrap();
    for (bytes, declared, reject, engine_error) in [
        (archive.as_slice(), archive.len() as u64, false, false),
        (archive.as_slice(), archive.len() as u64, true, true),
        (&b"not tar"[..], 7, true, false),
        (&archive[..900], 900, true, false),
        (archive.as_slice(), archive.len() as u64 + 1, true, false),
    ] {
        let before = state.imports.lock().await.len();
        state.import_failure.store(engine_error, Ordering::SeqCst);
        let mut file = tempfile::tempfile_in(directory).unwrap();
        file.write_all(bytes).unwrap();
        file.rewind().unwrap();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.materialize(file, declared),
        )
        .await
        .expect("invalid import must fail without waiting for staging inactivity");
        assert_eq!(outcome.is_err(), reject);
        let imports = state.imports.lock().await;
        if !reject || engine_error {
            assert_eq!(imports.len(), before + 1);
            assert_eq!(imports.last().unwrap(), &archive);
        } else {
            assert_eq!(imports.len(), before, "unverified archive reached Docker");
        }
    }
    assert_eq!(state.imports.lock().await.len(), 2);
    assert!(state.containers.lock().await.is_empty());
    assert_eq!(
        std::fs::read_dir(directory).unwrap().count(),
        directory_entries
    );
}

#[tokio::test]
async fn container_broker_anchor_response_failures_remain_fenced_after_engine_recovers() {
    use std::os::unix::fs::OpenOptionsExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tracing::instrument::WithSubscriber as _;

    const UPSTREAM_SECRET: &str = "private-container-control-token-canary";

    let mut oversized = b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n".to_vec();
    oversized.resize(oversized.len() + (1 << 20) + 1, b'x');
    for (response, expected_phase, expected_timeout) in [
        (
            Some(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 2\r\n\r\nOK".to_vec()),
            "status",
            false,
        ),
        (Some(oversized), "body", false),
        (
            Some(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{UPSTREAM_SECRET}",
                    UPSTREAM_SECRET.len()
                )
                .into_bytes(),
            ),
            "content",
            false,
        ),
        (None, "headers", true),
    ] {
        let directory = tempfile::Builder::new()
            .prefix("oc-cbf-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::fs::canonicalize(directory.path())
            .unwrap()
            .join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let requests = Arc::new(AtomicU64::new(0));
        let observed = requests.clone();
        let (recover, recovered) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for first in [true, false] {
                read_anchor_ping(&mut socket).await;
                observed.fetch_add(1, Ordering::Relaxed);
                if first {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                        .await
                        .unwrap();
                } else if let Some(response) = &response {
                    // A bounded-body rejection can close the connection before all bytes drain.
                    let _ = socket.write_all(response).await;
                }
            }
            recovered.await.unwrap();
            // A stalled Engine becomes responsive again; this cannot restore qualification.
            if response.is_none() {
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                    .await;
            }
            (listener, socket)
        });
        let log_path = directory.path().join("anchor.log");
        let log_file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&log_path)
            .unwrap();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::WARN)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(Arc::new(log_file))
            .finish();
        let subscriber = tracing::Dispatch::new(subscriber);
        let engine = super::super::engine::Engine::new(&path, "/_ping")
            .with_subscriber(subscriber.clone())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(7),
                engine.ensure_active().with_subscriber(subscriber.clone())
            )
            .await
            .unwrap()
            .is_err()
        );
        recover.send(()).unwrap();
        let (listener, mut socket) = server.await.unwrap();
        futures::future::join_all(
            (0..8).map(|_| engine.ensure_active().with_subscriber(subscriber.clone())),
        )
        .await
        .into_iter()
        .for_each(|result| assert!(result.is_err()));
        assert_eq!(requests.load(Ordering::Relaxed), 2);
        if let Ok(Ok(count)) = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            socket.read(&mut [0_u8; 1]),
        )
        .await
        {
            assert_eq!(count, 0, "dead anchor sent another request");
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "dead anchor reconnected to a recovered Engine"
        );
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(!log.contains(UPSTREAM_SECRET));
        assert!(!log.contains(path.to_str().unwrap()));
        assert!(!log.contains("reqwest"));
        assert!(!log.contains("hyper::Error"));
        let warnings: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["fields"]["message"] == "Container provider continuity lost")
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["level"], "WARN");
        assert_eq!(
            warnings[0]["fields"],
            json!({
                "message": "Container provider continuity lost",
                "reason": "probe_failed",
                "phase": expected_phase,
                "timeout": expected_timeout,
                "driver_failed": false,
            })
        );
        drop(engine);
    }
}

async fn read_anchor_ping(socket: &mut tokio::net::UnixStream) {
    use tokio::io::AsyncReadExt as _;

    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(socket.read_u8().await.unwrap());
        assert!(request.len() < 1024);
    }
    assert!(request.starts_with(b"GET /_ping HTTP/1.1\r\n"));
}

#[tokio::test]
async fn container_broker_anchor_caller_cancellation_preserves_connection() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let directory = tempfile::Builder::new()
        .prefix("oc-cbcancel-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::fs::canonicalize(directory.path())
        .unwrap()
        .join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let (partial, received_partial) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (closed, observed_closed) = oneshot::channel();
    let (finish, finished) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_anchor_ping(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
            .await
            .unwrap();
        read_anchor_ping(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nO")
            .await
            .unwrap();
        partial.send(()).unwrap();
        released.await.unwrap();
        let disconnected = matches!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                socket.read(&mut [0_u8; 1]),
            )
            .await,
            Ok(Ok(0))
        );
        closed.send(disconnected).unwrap();
        if disconnected {
            return;
        }
        socket.write_all(b"K").await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_anchor_ping(&mut socket),
        )
        .await
        .unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
            .await
            .unwrap();
        finished.await.unwrap();
    });
    let engine = super::super::engine::Engine::new(&path, "/_ping")
        .await
        .unwrap();
    let caller_engine = engine.clone();
    let caller = tokio::spawn(async move { caller_engine.ensure_active().await });
    received_partial.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    let disconnected = observed_closed.await.unwrap();
    if disconnected {
        server.await.unwrap();
        panic!("cancelling a caller closed the shared Engine connection");
    }
    engine.ensure_active().await.unwrap();
    finish.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn container_broker_anchor_connection_close_fails_without_reconnect() {
    use tokio::io::AsyncWriteExt as _;

    let directory = tempfile::Builder::new()
        .prefix("oc-cbc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::fs::canonicalize(directory.path())
        .unwrap()
        .join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let requests = Arc::new(AtomicU64::new(0));
    let observed = requests.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        for closing in [false, true] {
            read_anchor_ping(&mut socket).await;
            observed.fetch_add(1, Ordering::Relaxed);
            let response: &[u8] = if closing {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK"
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK"
            };
            socket.write_all(response).await.unwrap();
        }
        socket.shutdown().await.unwrap();
        listener
    });
    let engine = super::super::engine::Engine::new(&path, "/_ping")
        .await
        .unwrap();
    // The closing response can complete before the connection driver marks the anchor dead.
    let _ = engine.ensure_active().await;
    let listener = server.await.unwrap();
    assert!(engine.ensure_active().await.is_err());
    futures::future::join_all((0..8).map(|_| engine.ensure_active()))
        .await
        .into_iter()
        .for_each(|result| assert!(result.is_err()));
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    drop(engine);
    drop(listener);
}

#[tokio::test]
async fn container_broker_anchor_keepalive_and_socket_replacement_fail_closed() {
    let directory = tempfile::Builder::new()
        .prefix("oc-cba-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::fs::canonicalize(directory.path())
        .unwrap()
        .join("engine.sock");
    let state = Arc::new(MockEngine::default());
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let app = Router::new()
        .fallback(any(engine))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(TestListener(listener), app).await.unwrap();
    });
    let engine = super::super::engine::Engine::new(&path, "/_ping")
        .await
        .unwrap();
    futures::future::try_join_all((0..8).map(|_| engine.ensure_active()))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
    assert!(
        state
            .requests
            .lock()
            .await
            .iter()
            .filter(|request| request.as_str() == "GET /_ping")
            .count()
            >= 2
    );
    engine.ensure_active().await.unwrap();
    // Even if the original daemon and its accepted connection remain alive, rebinding fails closed.
    std::fs::remove_file(&path).unwrap();
    let replacement = tokio::net::UnixListener::bind(&path).unwrap();
    assert!(engine.ensure_active().await.is_err());
    drop(replacement);
    std::fs::remove_file(&path).unwrap();
    drop(engine);
    server.abort();
}
