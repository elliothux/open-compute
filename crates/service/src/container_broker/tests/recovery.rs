//! Real Unix Broker restarts preserve SQLite stop intent and conservative capacity.

use super::*;
use crate::containers::{ContainerService, test_support};
use open_compute_core::containers::ContainerLeaseState;
use open_compute_storage::containers::{
    ContainerApplicationMetadataPatch, ContainerAuthority, ContainerLeaseIdentity,
    ContainerRolloutCreate,
};

#[path = "directory.rs"]
mod directory;

#[path = "helper_input.rs"]
pub(in crate::container_broker::tests) mod helper_input;

#[path = "exec_transport.rs"]
mod exec_transport;
#[path = "sidecar_qualification.rs"]
mod sidecar_qualification;
#[path = "snapshot.rs"]
mod snapshot;
#[path = "startup_transport.rs"]
mod startup_transport;
#[path = "wait.rs"]
mod wait;

struct RestartBroker {
    _directory: tempfile::TempDir,
    config: BrokerConfig,
    state: Arc<MockEngine>,
    engine_task: tokio::task::JoinHandle<()>,
    sidecar_task: tokio::task::JoinHandle<()>,
    broker: Option<tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>>,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for RestartBroker {
    fn drop(&mut self) {
        if let Some(broker) = &self.broker {
            broker.abort();
        }
        self.engine_task.abort();
        self.sidecar_task.abort();
    }
}

impl RestartBroker {
    async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("oc-cbr-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let state = Arc::new(MockEngine::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        state
            .published_port
            .store(listener.local_addr().unwrap().port(), Ordering::Relaxed);
        let sidecar_state = state.clone();
        let sidecar_task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .fallback(any(sidecar))
                    .with_state(sidecar_state),
            )
            .await
            .unwrap();
        });
        let listener = tokio::net::UnixListener::bind(root.join("engine.sock")).unwrap();
        let engine_state = state.clone();
        let engine_task = tokio::spawn(async move {
            axum::serve(
                TestListener(listener),
                Router::new().fallback(any(engine)).with_state(engine_state),
            )
            .await
            .unwrap();
        });
        let mut config = config();
        config.socket = root.join("broker.sock");
        config.engine_socket = root.join("engine.sock");
        config.spool_dir = root.clone();
        config.state_dir = root.join("state");
        config.max_instances = 1;
        open_compute_storage::fs::ensure_dir_secure(&config.state_dir).unwrap();
        Self {
            _directory: directory,
            config,
            state,
            engine_task,
            sidecar_task,
            broker: None,
            stop: None,
        }
    }

    async fn start(&mut self) -> QualifiedContainerProvider {
        let (stop, shutdown) = oneshot::channel();
        let config = self.config.clone();
        self.stop = Some(stop);
        self.broker = Some(tokio::spawn(async move {
            run_broker(config, async {
                let _ = shutdown.await;
            })
            .await
        }));
        let client = self.client();
        let contract = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(response) = client
                    .get("http://localhost/open-compute/v1/contract")
                    .send()
                    .await
                    && response.status() == 200
                {
                    break serde_json::from_slice::<ProviderContract>(
                        &response.bytes().await.unwrap(),
                    )
                    .unwrap();
                }
                assert!(!self.broker.as_ref().unwrap().is_finished());
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(contract.qualified);
        QualifiedContainerProvider::preflight(&open_compute_core::config::ContainersConfig {
            broker_socket: self.config.socket.clone(),
            provider_contract_sha256: contract.sha256().unwrap(),
            max_snapshots: 32,
            max_snapshot_bytes: 8 << 30,
            max_instances: 1,
            resources: resources(),
        })
        .await
        .unwrap()
    }

    async fn retire(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.broker.take().unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(!self.config.socket.exists());
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .unix_socket(self.config.socket.clone())
            .no_proxy()
            .build()
            .unwrap()
    }

    async fn image(&self, fixture: &test_support::Fixture, digest: &[u8; 32]) {
        let image = ContainerAuthority::new(fixture.service.storage.db())
            .image(fixture.service.instance_id(), digest)
            .unwrap();
        let id = format!("sha256:{}", hex::encode(image.config_sha256));
        self.state.image_overrides.lock().await.insert(id.clone(), (200, json!({
            "Id":id,"Os":"linux","Architecture":"amd64","Config":image_config(),
            "Size":73,"RootFS":{"Type":"layers","Layers":image.diff_ids.iter().map(|id| format!("sha256:{}",hex::encode(id))).collect::<Vec<_>>()}
        })));
    }

    async fn execute(&self, lease: &crate::containers::ResolvedLease) {
        let client = self.client();
        for proxy in [true, false] {
            let name = format!(
                "{}{}",
                lease.opaque_engine_key,
                if proxy { "-proxy" } else { "" }
            );
            let body = if proxy {
                create(true)
            } else {
                json!({"Image":lease.image_ref,"Env":[],"HostConfig":{
                    "NetworkMode":format!("container:{}-proxy",lease.opaque_engine_key),
                    "RestartPolicy":{"Name":"no"},"CapDrop":["NET_RAW"]}})
            };
            assert_eq!(
                client
                    .post(format!("http://localhost/containers/create?name={name}"))
                    .body(serde_json::to_vec(&body).unwrap())
                    .send()
                    .await
                    .unwrap()
                    .status(),
                201
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
        }
    }
}

fn use_provider(fixture: &mut test_support::Fixture, provider: QualifiedContainerProvider) {
    fixture.service = Arc::new(ContainerService::new(
        fixture.service.storage.clone(),
        fixture.service.artifacts.clone(),
        fixture.service.cache.clone(),
        provider,
    ));
}

#[derive(Clone, Copy)]
enum Pending {
    NativeStop,
    Rollout,
    Delete,
}

#[tokio::test]
async fn container_broker_restart_adopts_pending_stop_rollout_and_delete_without_new_grants() {
    for pending in [Pending::NativeStop, Pending::Rollout, Pending::Delete] {
        pending_cleanup(pending).await;
    }
}

async fn pending_cleanup(pending: Pending) {
    let mut broker = RestartBroker::new().await;
    let provider = broker.start().await;
    let mut fixture = test_support::fixture().await;
    fixture.broker.abort();
    use_provider(&mut fixture, provider);
    let image = fixture.admit_image("original").await;
    broker.image(&fixture, &image).await;
    let authority = test_support::authority(&fixture.service, &fixture._artifacts, image, true);
    let (auth, credential) = test_support::generation();
    let initial = fixture
        .service
        .resolve(authority.request(true, None), 10, &auth, &credential)
        .await
        .unwrap()
        .lease
        .unwrap();
    broker.execute(&initial).await;
    fixture
        .service
        .resolve(authority.request(false, None), 11, &auth, &credential)
        .await
        .unwrap();
    let identity = ContainerLeaseIdentity {
        namespace_resource_id: authority.object.namespace_resource_id,
        object_id: authority.object.object_id,
        object_generation: 1,
        lease_generation: initial.lease_generation,
    };
    let instance = fixture.service.instance_id();
    let rollout = if matches!(pending, Pending::Rollout) {
        let image = fixture.admit_image("replacement").await;
        broker.image(&fixture, &image).await;
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        let mut spec = repo.target(instance, authority.app.id, 1).unwrap().spec;
        spec.image_sha256 = Some(image);
        let (mut configuration, _, _) = fixture
            .service
            .application_configuration(&authority.app)
            .unwrap();
        configuration.image = format!(
            "registry.example.test/{instance}/authority-app@sha256:{}",
            hex::encode(image)
        );
        let bytes = serde_json::to_vec(&configuration).unwrap();
        repo.commit_application_configuration(
            instance,
            &authority.app,
            &spec,
            &bytes,
            ContainerApplicationMetadataPatch::default(),
            12,
        )
        .unwrap();
        let id = open_compute_core::ResourceId::generate();
        repo.create_rollout(
            instance,
            authority.app.id,
            &ContainerRolloutCreate {
                id,
                target_configuration: &bytes,
                steps: &[100],
                details: None,
            },
            13,
        )
        .unwrap();
        repo.rollout_step_leases(instance, authority.app.id, id, 14)
            .unwrap();
        Some(id)
    } else {
        let repo = ContainerAuthority::new(fixture.service.storage.db());
        if matches!(pending, Pending::Delete) {
            repo.begin_application_delete(instance, authority.app.id, 14)
                .unwrap();
        } else {
            fixture
                .service
                .begin_stop(&authority.stop(&initial), 14, &auth, &credential)
                .unwrap();
        }
        None
    };
    broker.retire().await;
    use_provider(&mut fixture, broker.start().await);
    let client = broker.client();
    let registration = test_support::registration(&fixture.service, identity);
    // A forged immutable execution cannot be adopted, even for cleanup.
    let mut forged = registration.clone();
    forged.owner_key = "0".repeat(64);
    assert!(
        fixture
            .service
            .provider
            .adopt_for_cleanup(&forged)
            .await
            .is_err()
    );
    let before = broker.state.requests.lock().await.len();
    broker.state.delete_failure.store(true, Ordering::SeqCst);
    let transport = crate::runtime_bridge::WorkerdTransport::for_test_endpoint(auth.clone(), 9);
    if matches!(pending, Pending::NativeStop) {
        let mut request = authority.request(false, None);
        request.for_stop = true;
        assert!(
            fixture
                .service
                .resolve(request, 15, &auth, &credential)
                .await
                .is_err()
        );
    } else {
        assert!(
            fixture
                .service
                .reconcile_rollouts(&transport)
                .await
                .is_err()
        );
    }
    let storage = fixture.service.storage.clone();
    let repo = ContainerAuthority::new(storage.db());
    let uncertain = repo.lease(instance, identity).unwrap();
    assert_eq!(uncertain.state, ContainerLeaseState::Lost);
    assert_eq!(uncertain.stop_requested_at_ms, Some(14));
    assert_eq!(broker.state.containers.lock().await.len(), 2);
    assert!(
        fixture
            .service
            .provider
            .lease_credentials(&initial.opaque_engine_key, &"1".repeat(64))
            .await
            .is_err()
    );
    assert!(
        fixture
            .service
            .provider
            .register(&registration)
            .await
            .is_err()
    );
    let mut changed = registration.clone();
    changed.startup.environment_variables = vec!["UNTRUSTED=changed".into()];
    assert!(
        fixture
            .service
            .provider
            .adopt_for_cleanup(&changed)
            .await
            .is_err()
    );
    let mut competing = registration.clone();
    competing.lease_key = "8".repeat(64);
    assert!(fixture.service.provider.register(&competing).await.is_err());
    assert_eq!(
        client
            .post(format!(
                "http://localhost/containers/{}/start",
                initial.opaque_engine_key
            ))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    let other =
        test_support::insert_object(&fixture.service, authority.object.namespace_resource_id, 9);
    let mut request = authority.request(true, None);
    request.object_id = other;
    assert!(
        fixture
            .service
            .resolve(request, 16, &auth, &credential)
            .await
            .is_err()
    );
    // Stop intent survives another OCD service construction. Retrying a fenced entry must
    // not pass through normal admission or manufacture a new credential generation.
    fixture.service = Arc::new(fixture.restart_service());
    if matches!(pending, Pending::NativeStop) {
        // No new native request is needed: the ordinary maintenance tick owns recovery.
        fixture
            .service
            .reconcile_rollouts(&transport)
            .await
            .unwrap();
        let mut request = authority.request(false, None);
        request.for_stop = true;
        assert!(
            fixture
                .service
                .resolve(request, 17, &auth, &credential)
                .await
                .unwrap()
                .lease
                .is_none()
        );
        let requests = broker
            .state
            .requests
            .lock()
            .await
            .iter()
            .filter(|request| request.as_str() != "GET /_ping")
            .count();
        fixture
            .service
            .begin_stop(&authority.stop(&initial), 17, &auth, &credential)
            .unwrap();
        fixture
            .service
            .stop(&authority.stop(&initial), 17, &auth, &credential)
            .await
            .unwrap();
        assert_eq!(
            broker
                .state
                .requests
                .lock()
                .await
                .iter()
                .filter(|request| request.as_str() != "GET /_ping")
                .count(),
            requests
        );
        for name in [
            initial.opaque_engine_key.clone(),
            format!("{}-proxy", initial.opaque_engine_key),
        ] {
            assert_eq!(
                client
                    .delete(format!("http://localhost/containers/{name}?force=true"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
        }
        fixture
            .service
            .stop(&authority.stop(&initial), 17, &auth, &credential)
            .await
            .unwrap();
    } else if matches!(pending, Pending::Delete) {
        fixture
            .service
            .reconcile_rollouts(&transport)
            .await
            .unwrap();
        assert!(
            repo.pending_application_deletions(instance)
                .unwrap()
                .is_empty()
        );
        assert!(repo.applications(instance).unwrap().is_empty());
    } else {
        // The intentionally absent native endpoint cannot start the replacement, but it
        // must not prevent positive old-execution cleanup or release capacity early.
        assert!(
            fixture
                .service
                .reconcile_rollouts(&transport)
                .await
                .is_err()
        );
        assert_eq!(
            repo.rollout_progress(instance, authority.app.id, rollout.unwrap())
                .unwrap(),
            (1, 0)
        );
        assert_eq!(
            repo.rollout(instance, authority.app.id, rollout.unwrap())
                .unwrap()
                .state,
            "running"
        );
    }
    assert_eq!(
        repo.lease(instance, identity).unwrap().state,
        ContainerLeaseState::Stopped
    );
    assert!(broker.state.containers.lock().await.is_empty());
    let requests = broker.state.requests.lock().await;
    assert!(!requests[before..].iter().any(
        |request| request.starts_with("PUT ") || request.starts_with("POST /containers/create")
    ));
    drop(requests);
    if !matches!(pending, Pending::Delete) {
        let replacement = fixture
            .service
            .resolve(authority.request(true, None), 18, &auth, &credential)
            .await
            .unwrap()
            .lease
            .unwrap();
        assert_ne!(replacement.opaque_engine_key, initial.opaque_engine_key);
        assert_eq!(replacement.lease_generation, initial.lease_generation + 1);
        assert!(
            fixture
                .service
                .stop(&authority.stop(&initial), 18, &auth, &credential)
                .await
                .is_err()
        );
        broker.execute(&replacement).await;
        fixture
            .service
            .resolve(authority.request(false, None), 19, &auth, &credential)
            .await
            .unwrap();
        fixture
            .service
            .stop(&authority.stop(&replacement), 20, &auth, &credential)
            .await
            .unwrap();
        assert!(broker.state.containers.lock().await.is_empty());
    }
    assert!(broker.state.imports.lock().await.is_empty());
    broker.retire().await;
}
