//! Real SQLite/artifact fixtures and explicitly simulated Broker protocol for unit tests.

use super::ContainerService;
use super::*;
use crate::{
    container_broker::{ProviderContract, QualifiedContainerProvider},
    http::HttpState,
    runtime_feature_fixture::RuntimeFeatureFixture,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{StatusCode, header},
    response::Response,
};
use open_compute_artifacts::ContainerImageLimits;
use open_compute_core::{BindingKind, RequestId, VersionId, durable_object_namespace_prefix};
use open_compute_core::{config::ContainersConfig, containers::ContainerResources};
use open_compute_storage::{
    containers::{ContainerApplication, ContainerTargetSpec, VersionContainerDeclaration},
    durable_objects::{DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository},
    resources::{ReserveResourceCreate, ResourceCreateReservation, ResourceRepository},
    worker_repository::{NewVersion, NewVersionProducts, WorkerRepository},
};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt as _, sync::Arc};

pub(crate) mod snapshots;

pub(crate) struct Authority {
    pub(crate) app: ContainerApplication,
    pub(crate) object: ContainerObjectAuthority,
}

impl Authority {
    pub(crate) fn request(&self, start: bool, startup: Option<Value>) -> ResolveRequest {
        let mut value = json!({"instanceId":self.object.instance_id,
            "namespaceResourceId":self.object.namespace_resource_id,"applicationName":self.app.name,
            "objectId":self.object.object_id,"objectGeneration":self.object.object_generation,
            "routeGeneration":self.object.route_generation,"descriptorSha256":hex::encode(self.object.descriptor_sha256),
            "start":start,"forStop":false,"forMonitor":false});
        value["startup"] = startup.unwrap_or(Value::Null);
        serde_json::from_value(value).unwrap()
    }
    pub(crate) fn stop(&self, lease: &ResolvedLease) -> StopRequest {
        serde_json::from_value(json!({"instanceId":self.object.instance_id,
            "namespaceResourceId":self.object.namespace_resource_id,"applicationName":self.app.name,
            "objectId":self.object.object_id,"objectGeneration":self.object.object_generation,
            "routeGeneration":self.object.route_generation,"descriptorSha256":hex::encode(self.object.descriptor_sha256),
            "leaseGeneration":lease.lease_generation,"opaqueEngineKey":lease.opaque_engine_key})).unwrap()
    }
}

pub(crate) fn declared_namespace(
    service: &ContainerService,
    artifacts: &RuntimeFeatureFixture,
    images: std::collections::BTreeMap<String, String>,
) -> (VersionContainerDeclaration, VersionId, u64) {
    let storage = &service.storage;
    let instance = service.instance_id();
    let namespace = ResourceId::generate();
    let resources = ResourceRepository::new(storage.db());
    let ResourceCreateReservation::Reserved(resource) = resources
        .reserve_create(
            &ReserveResourceCreate {
                instance_id: instance,
                kind: BindingKind::DoNamespace,
                name: "ContainerClass",
                idempotency_key: "authority-namespace",
                fingerprint_key_id: "fixture",
                request_fingerprint: &[1; 32],
                resource_id: namespace,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 1,
                expires_at_ms: 1000,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("namespace reservation")
    };
    DurableObjectRepository::new(storage)
        .ensure_namespace(&resource, artifacts.worker, "ContainerClass")
        .unwrap();
    resources.mark_ready(namespace, 2).unwrap();
    let declaration =
        VersionContainerDeclaration::with_images(namespace, "authority-app".into(), images)
            .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let original = workers
        .get_version(instance, artifacts.worker, artifacts.version)
        .unwrap();
    // The authority protocol fixture admits its products in a fresh staging Version. It does
    // not mutate the ready runtime-feature fixture or claim to validate a running Worker.
    let version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                instance_id: instance,
                worker_id: artifacts.worker,
                content_kind: original.content_kind,
                artifact_sha256: original.artifact_sha256,
                artifact_size: original.artifact_size,
                artifact_schema_version: original.artifact_schema_version,
                main_module: original.main_module,
                worker_code_sha256: original.worker_code_sha256,
                compatibility_date: original.compatibility_date,
                compatibility_flags: original.compatibility_flags,
                resource_limits: original.resource_limits,
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 3,
            },
            &NewVersionProducts {
                containers: std::slice::from_ref(&declaration),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 4).unwrap();
    let route = workers
        .promote(
            instance,
            artifacts.worker,
            version,
            None,
            RequestId::generate(),
            5,
        )
        .unwrap()
        .route_generation;
    (declaration, version, route)
}

pub(crate) fn authority(
    service: &ContainerService,
    artifacts: &RuntimeFeatureFixture,
    image: [u8; 32],
    internet: bool,
) -> Authority {
    let (declaration, version, route) = declared_namespace(service, artifacts, Default::default());
    let namespace = declaration.namespace_resource_id;
    let storage = &service.storage;
    let instance = service.instance_id();
    let repo = ContainerAuthority::new(storage.db());
    let app = ResourceId::generate();
    repo.create_application(instance, app, namespace, "authority-app", 1, 6)
        .unwrap();
    let target = ContainerTargetSpec {
        worker_version_id: version,
        image_sha256: Some(image),
        provider_contract_sha256: digest(service.provider.contract_sha256()).unwrap(),
        resources: service.provider.resources(),
        enable_internet: internet,
    };
    let configuration = wire::Configuration {
        image: format!(
            "registry.example.test/{instance}/authority-app@sha256:{}",
            hex::encode(image)
        ),
        instance_type: None,
        vcpu: Some(target.resources.cpu_nanos as f64 / 1e9),
        memory_mib: Some(target.resources.memory_bytes / (1 << 20)),
        disk: Some(wire::Disk {
            size_mb: target.resources.disk_bytes as f64 / 1e6,
        }),
    };
    let application = repo
        .application_for_namespace(instance, namespace, "authority-app")
        .unwrap();
    repo.commit_application_configuration(
        instance,
        &application,
        &target,
        &serde_json::to_vec(&configuration).unwrap(),
        open_compute_storage::containers::ContainerApplicationMetadataPatch {
            max_instances: None,
            observability_logs: Some(false),
        },
        7,
    )
    .unwrap();
    repo.set_admission_paused(instance, app, false).unwrap();
    let object = insert_object(service, namespace, 7);
    Authority {
        app: repo
            .application_for_namespace(instance, namespace, "authority-app")
            .unwrap(),
        object: ContainerObjectAuthority {
            instance_id: instance,
            application_id: app,
            namespace_resource_id: namespace,
            object_id: object,
            object_generation: 1,
            route_generation: route,
            descriptor_sha256: declaration.descriptor_sha256,
        },
    }
}

pub(crate) fn do_authority(
    service: &ContainerService,
    artifacts: &RuntimeFeatureFixture,
    image: [u8; 32],
    internet: bool,
) -> Authority {
    do_authority_with_resources(
        service,
        artifacts,
        image,
        internet,
        service.provider.resources(),
    )
}

pub(crate) fn do_authority_with_resources(
    service: &ContainerService,
    artifacts: &RuntimeFeatureFixture,
    image: [u8; 32],
    internet: bool,
    resources: ContainerResources,
) -> Authority {
    use open_compute_storage::containers::ContainerSchedulingPolicy;
    let reference = format!("registry.example.test/app@sha256:{}", hex::encode(image));
    let (declaration, version, route) = declared_namespace(
        service,
        artifacts,
        std::collections::BTreeMap::from([("main".into(), reference)]),
    );
    let namespace = declaration.namespace_resource_id;
    let instance = service.instance_id();
    let repo = ContainerAuthority::new(service.storage.db());
    let app = ContainerApplication {
        id: namespace,
        namespace_resource_id: namespace,
        name: "authority-app".into(),
        scheduling_policy: ContainerSchedulingPolicy::DurableObject,
        max_instances: 1,
        target_generation: None,
        admission_paused: true,
        created_at_ms: 6,
    };
    repo.create_durable_object_application(
        instance,
        &app,
        &ContainerTargetSpec {
            worker_version_id: version,
            image_sha256: None,
            provider_contract_sha256: digest(service.provider.contract_sha256()).unwrap(),
            resources,
            enable_internet: internet,
        },
        false,
        7,
    )
    .unwrap();
    Authority {
        app: repo
            .application_for_namespace(instance, namespace, "authority-app")
            .unwrap(),
        object: ContainerObjectAuthority {
            instance_id: instance,
            application_id: namespace,
            namespace_resource_id: namespace,
            object_id: insert_object(service, namespace, 7),
            object_generation: 1,
            route_generation: route,
            descriptor_sha256: declaration.descriptor_sha256,
        },
    }
}

pub(crate) fn insert_object(
    service: &ContainerService,
    namespace: ResourceId,
    fill: u8,
) -> DurableObjectId {
    let mut bytes = [fill; 32];
    bytes[..8].copy_from_slice(&durable_object_namespace_prefix(namespace));
    let object = DurableObjectId::for_namespace(bytes, namespace).unwrap();
    let mut connection =
        rusqlite::Connection::open(service.storage.data_dir().root().join("control.sqlite"))
            .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    transaction.execute("INSERT INTO do_objects(namespace_resource_id,object_id,generation,state,created_at_ms,updated_at_ms) VALUES(?1,?2,1,'ready',8,8)", rusqlite::params![namespace,object.to_string()]).unwrap();
    transaction.commit().unwrap();
    object
}

pub(crate) fn registration(
    service: &ContainerService,
    identity: ContainerLeaseIdentity,
) -> BrokerLease {
    let lease = ContainerAuthority::new(service.storage.db())
        .lease(service.instance_id(), identity)
        .unwrap();
    service.registration(&lease).unwrap()
}

pub(crate) struct Fixture {
    pub(crate) _artifacts: RuntimeFeatureFixture,
    pub(crate) _socket: tempfile::TempDir,
    pub(crate) broker: tokio::task::JoinHandle<()>,
    pub(crate) shutdown: Arc<tokio::sync::Notify>,
    pub(crate) state: HttpState,
    pub(crate) service: Arc<ContainerService>,
    pub(crate) credentials: Arc<CredentialProtocol>,
    pub(crate) protocol: Arc<LeaseProtocol>,
}

#[derive(Default)]
pub(crate) struct LeaseProtocol {
    pub(crate) registrations: tokio::sync::Mutex<Vec<BrokerLease>>,
    pub(crate) cleanup_registrations: tokio::sync::Mutex<Vec<BrokerLease>>,
    pub(crate) images: tokio::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>,
    pub(crate) image_size: std::sync::atomic::AtomicU64,
    pub(crate) image_checks: tokio::sync::Mutex<Vec<String>>,
    pub(crate) imports: std::sync::atomic::AtomicUsize,
    pub(crate) import_fail: std::sync::atomic::AtomicBool,
    pub(crate) import_proof_absent: std::sync::atomic::AtomicBool,
    pub(crate) observation:
        tokio::sync::Mutex<Option<crate::container_broker::ProviderLeaseObservation>>,
    pub(crate) observation_key: tokio::sync::Mutex<Option<String>>,
    pub(crate) directory_archives: tokio::sync::Mutex<Vec<(String, usize, Vec<u8>)>>,
    pub(crate) register_fail: std::sync::atomic::AtomicBool,
    pub(crate) terminate_fail: std::sync::atomic::AtomicBool,
    pub(crate) terminated: tokio::sync::Mutex<Vec<String>>,
    pub(crate) stop_grace_millis: tokio::sync::Mutex<Vec<u64>>,
    pub(crate) snapshot_rootfs: tokio::sync::Mutex<Vec<u8>>,
    pub(crate) snapshot_requests: tokio::sync::Mutex<Vec<(String, Value)>>,
    pub(crate) snapshot_block: std::sync::atomic::AtomicBool,
    pub(crate) snapshot_entered: tokio::sync::Notify,
    pub(crate) snapshot_release: tokio::sync::Notify,
    pub(crate) snapshot_fail: std::sync::atomic::AtomicBool,
}
#[derive(Default)]
pub(crate) struct CredentialProtocol {
    pub(crate) observed: tokio::sync::Mutex<Vec<String>>,
    pub(crate) entered: tokio::sync::Notify,
    pub(crate) release: tokio::sync::Notify,
    pub(crate) block: std::sync::atomic::AtomicBool,
    pub(crate) fail: std::sync::atomic::AtomicBool,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        self.broker.abort();
    }
}

impl Fixture {
    pub(crate) fn restart_service(&self) -> ContainerService {
        ContainerService::new(
            Arc::clone(&self.service.storage),
            self.service.artifacts.clone(),
            Arc::clone(&self.service.cache),
            self.service.provider.clone(),
        )
    }
    pub(crate) async fn admit_image(&self, content: &str) -> [u8; 32] {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, "app", content.as_bytes())
            .unwrap();
        let layer = builder.into_inner().unwrap();
        let layer_digest = hex::encode(Sha256::digest(&layer));
        let config=serde_json::to_vec(&json!({"os":"linux","architecture":self.service.provider.architecture(),"rootfs":{"type":"layers","diff_ids":[format!("sha256:{layer_digest}")]}})).unwrap();
        let config_digest = hex::encode(Sha256::digest(&config));
        let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":format!("sha256:{config_digest}"),"size":config.len()},"layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":format!("sha256:{layer_digest}"),"size":layer.len()}]})).unwrap();
        let mut blobs = Vec::new();
        for bytes in [config, layer] {
            let digest = hex::encode(Sha256::digest(&bytes));
            let size = bytes.len() as u64;
            blobs.push(
                self.service
                    .artifacts
                    .put_verified(
                        futures::stream::once(async {
                            Ok::<_, std::io::Error>(bytes::Bytes::from(bytes))
                        }),
                        &digest,
                        size,
                    )
                    .await
                    .unwrap(),
            );
        }
        let expected = Sha256::digest(&manifest).into();
        self.service
            .admit_image(crate::container_images::ContainerImageUpload {
                manifest: &manifest,
                expected,
                architecture: self.service.provider.architecture(),
                blobs,
                limits: ContainerImageLimits {
                    json_bytes: 8192,
                    compressed_bytes: 8192,
                    unpacked_bytes: 8192,
                    layers: 8,
                    entries: 8,
                },
            })
            .await
            .unwrap();
        expected
    }
    pub(crate) async fn running(&self) {
        let registration = self
            .protocol
            .registrations
            .lock()
            .await
            .last()
            .unwrap()
            .clone();
        let staged = self.protocol.directory_archives.lock().await;
        assert!(
            registration
                .directory_snapshots
                .iter()
                .enumerate()
                .all(|(index, directory)| directory.archive.is_none()
                    || staged
                        .iter()
                        .any(|(key, position, _)| key == &registration.lease_key
                            && *position == index))
        );
        *self.protocol.observation_key.lock().await = Some(registration.lease_key.clone());
        *self.protocol.observation.lock().await =
            Some(crate::container_broker::ProviderLeaseObservation {
                directories_prepared: true,
                start_identity: "c".repeat(64),
                image: registration.image,
                manifest_sha256: registration.manifest_sha256,
                provider_contract_sha256: registration.provider_contract_sha256,
                running: true,
                terminated: false,
                cleanup_complete: false,
                late_operations_fenced: false,
            });
    }
}

struct Listener(tokio::net::UnixListener);
impl axum::serve::Listener for Listener {
    type Io = tokio::net::UnixStream;
    type Addr = tokio::net::unix::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        self.0.accept().await.unwrap()
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

pub(crate) async fn fixture() -> Fixture {
    let artifacts = RuntimeFeatureFixture::create(open_compute_workers::VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".to_owned(),
        ..Default::default()
    })
    .await;
    let directory = tempfile::Builder::new()
        .prefix("oc-crt-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = std::fs::canonicalize(directory.path())
        .unwrap()
        .join("broker.sock");
    let resources = ContainerResources {
        cpu_nanos: 1_000_000_000,
        memory_bytes: 1 << 30,
        disk_bytes: 4_000_000_000,
        pids: 64,
        open_files: 1024,
    };
    let contract = ProviderContract {
        version: ProviderContract::VERSION,
        runtime: "docker".into(),
        engine_version: "test protocol fixture".into(),
        architecture: "arm64".into(),
        isolation: "test protocol fixture".into(),
        qualified: true,
        snapshots: true,
        sidecar_image: Some(format!("sha256:{}", "b".repeat(64))),
        exec_helper_sha256: "1".repeat(64),
        resources,
        max_instances: 4,
        max_image_bytes: 8 << 20,
        max_stream_bytes: 64 << 10,
    };
    let json_contract = contract.clone();
    let credentials = Arc::new(CredentialProtocol::default());
    let credential_state = Arc::clone(&credentials);
    let protocol = Arc::new(LeaseProtocol::default());
    let register_protocol = Arc::clone(&protocol);
    let cleanup_protocol = Arc::clone(&protocol);
    let observe_protocol = Arc::clone(&protocol);
    let terminate_protocol = Arc::clone(&protocol);
    let grace_protocol = Arc::clone(&protocol);
    let image_protocol = Arc::clone(&protocol);
    let load_protocol = Arc::clone(&protocol);
    let snapshot_protocol = Arc::clone(&protocol);
    let directory_protocol = Arc::clone(&protocol);
    let app = Router::new()
        .route(
            "/open-compute/v1/contract",
            axum::routing::get(move || {
                let value = json_contract.clone();
                async move { axum::Json(value) }
            }),
        )
        .route("/open-compute/v1/leases/{key}/directories/{index}",axum::routing::post(move |
            axum::extract::Path((key,index)): axum::extract::Path<(String,usize)>, request: Request| {
            let protocol=Arc::clone(&directory_protocol);
            async move {
                let bytes=to_bytes(request.into_body(),8<<20).await.unwrap().to_vec();
                let registrations=protocol.registrations.lock().await;
                let registration=registrations.iter().rev().find(|entry|entry.lease_key==key).unwrap();
                let expected=registration.directory_snapshots[index].archive.as_ref().unwrap();
                assert_eq!(expected.bytes,bytes.len() as u64);
                assert_eq!(expected.sha256,hex::encode(Sha256::digest(&bytes)));
                protocol.directory_archives.lock().await.push((key,index,bytes));
                StatusCode::NO_CONTENT
            }
        }))
        .route("/open-compute/v1/leases/{key}/snapshot", axum::routing::post(move |
            axum::extract::Path(key): axum::extract::Path<String>,
            axum::Json(value): axum::Json<Value>,
        | snapshots::capture(Arc::clone(&snapshot_protocol), key, value)))
        .route("/open-compute/v1/leases/{key}/credentials",axum::routing::post(move |axum::Json(value):axum::Json<Value>| {
            let protocol=Arc::clone(&credential_state);
            async move {
                protocol.observed.lock().await.push(value["generationSha256"].as_str().unwrap().to_owned());
                if protocol.block.swap(false,std::sync::atomic::Ordering::SeqCst) {protocol.entered.notify_one();protocol.release.notified().await;}
                if protocol.fail.load(std::sync::atomic::Ordering::SeqCst) {return Response::builder().status(StatusCode::SERVICE_UNAVAILABLE).body(Body::empty()).unwrap();}
                Response::builder().header(header::CONTENT_TYPE,"application/json").body(Body::from(serde_json::to_vec(&serde_json::json!({"controlToken":"1".repeat(64),"callbackToken":"2".repeat(64)})).unwrap())).unwrap()
            }
        }))
        .route("/open-compute/v1/leases",axum::routing::post(move |axum::Json(value):axum::Json<BrokerLease>| {
            let protocol=Arc::clone(&register_protocol);
            async move {
                protocol.registrations.lock().await.push(value);
                if protocol.register_fail.load(std::sync::atomic::Ordering::SeqCst) {StatusCode::SERVICE_UNAVAILABLE} else {StatusCode::CREATED}
            }
        }))
        .route("/open-compute/v1/leases/cleanup",axum::routing::post(move |axum::Json(value):axum::Json<BrokerLease>| {
            let protocol=Arc::clone(&cleanup_protocol);
            async move {
                protocol.cleanup_registrations.lock().await.push(value);
                if protocol.register_fail.load(std::sync::atomic::Ordering::SeqCst) {StatusCode::SERVICE_UNAVAILABLE} else {StatusCode::CREATED}
            }
        }))
        .route("/open-compute/v1/leases/{key}",axum::routing::get(move |axum::extract::Path(key):axum::extract::Path<String>| {
            let protocol=Arc::clone(&observe_protocol);
            async move {
                if protocol.observation_key.lock().await.as_ref().is_none_or(|observed|observed==&key)
                    && let Some(value)=protocol.observation.lock().await.clone() {
                    return axum::response::IntoResponse::into_response(axum::Json(value));
                }
                let registrations=protocol.registrations.lock().await;
                match registrations.iter().rev().find(|entry|entry.lease_key==key && !entry.directory_snapshots.is_empty()) {
                    Some(registration)=>axum::response::IntoResponse::into_response(axum::Json(crate::container_broker::ProviderLeaseObservation {
                        directories_prepared:false,start_identity:registration.start_identity.clone(),
                        image:registration.image.clone(),manifest_sha256:registration.manifest_sha256.clone(),
                        provider_contract_sha256:registration.provider_contract_sha256.clone(),
                        running:false,terminated:false,cleanup_complete:false,late_operations_fenced:false,
                    })),
                    None=>axum::response::IntoResponse::into_response(StatusCode::SERVICE_UNAVAILABLE),
                }
            }
        }).delete(move |axum::extract::Path(key):axum::extract::Path<String>| {
            let protocol=Arc::clone(&terminate_protocol);
            async move {
                protocol.terminated.lock().await.push(key);
                if protocol.terminate_fail.load(std::sync::atomic::Ordering::SeqCst) {StatusCode::SERVICE_UNAVAILABLE} else {StatusCode::NO_CONTENT}
            }
        }))
        .route("/open-compute/v1/leases/{key}/stop",axum::routing::post(move |axum::extract::Path(key):axum::extract::Path<String>,axum::Json(value):axum::Json<Value>| {
            let protocol=Arc::clone(&grace_protocol);
            async move {
                assert_eq!(value.as_object().unwrap().len(),1);
                let grace = value["graceMillis"].as_u64().unwrap();
                assert!(grace<=900_000);
                protocol.stop_grace_millis.lock().await.push(grace);
                protocol.terminated.lock().await.push(key);
                if protocol.terminate_fail.load(std::sync::atomic::Ordering::SeqCst) {StatusCode::SERVICE_UNAVAILABLE} else {StatusCode::NO_CONTENT}
            }
        }))
        .route("/open-compute/v1/images/check", axum::routing::post(move |axum::Json(value):axum::Json<Value>| {
            let protocol=Arc::clone(&image_protocol);
            async move {
                use axum::response::IntoResponse as _;
                let image=value["image"].as_str().unwrap();
                protocol.image_checks.lock().await.push(image.to_owned());
                let images=protocol.images.lock().await;
                match images.get(image) {
                    None => StatusCode::NOT_FOUND.into_response(),
                    Some(ids) if serde_json::to_value(ids).unwrap()==value["diffIds"] => axum::Json(serde_json::json!({"size":protocol.image_size.load(std::sync::atomic::Ordering::SeqCst)})).into_response(),
                    Some(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                }
            }
        }))
        .route("/open-compute/v1/images/load", axum::routing::post(move |request: Request| {
            let protocol=Arc::clone(&load_protocol);
            async move {
                use std::io::Read as _;
                protocol.imports.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                let bytes=to_bytes(request.into_body(),8<<20).await.unwrap();
                if protocol.import_fail.load(std::sync::atomic::Ordering::SeqCst) { return StatusCode::SERVICE_UNAVAILABLE; }
                let mut archive=tar::Archive::new(bytes.as_ref());
                let mut members=std::collections::BTreeMap::new();
                for entry in archive.entries().unwrap() {
                    let mut entry=entry.unwrap();
                    let path=entry.path().unwrap().into_owned();
                    let mut contents=Vec::new();
                    entry.read_to_end(&mut contents).unwrap();
                    members.insert(path,contents);
                }
                let manifest:Value=serde_json::from_slice(&members[std::path::Path::new("manifest.json")]).unwrap();
                let config=&members[std::path::Path::new(manifest[0]["Config"].as_str().unwrap())];
                let id=format!("sha256:{}",hex::encode(Sha256::digest(config)));
                let config:Value=serde_json::from_slice(config).unwrap();
                let ids=serde_json::from_value(config["rootfs"]["diff_ids"].clone()).unwrap();
                if !protocol.import_proof_absent.load(std::sync::atomic::Ordering::SeqCst) {
                    protocol.images.lock().await.insert(id,ids);
                }
                StatusCode::OK
            }
        }));
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&shutdown);
    let broker = tokio::spawn(async move {
        axum::serve(Listener(listener), app)
            .with_graceful_shutdown(async move { signal.notified().await })
            .await
            .unwrap();
    });
    let provider = QualifiedContainerProvider::preflight(&ContainersConfig {
        broker_socket: socket,
        provider_contract_sha256: contract.sha256().unwrap(),
        max_instances: 4,
        max_snapshots: 32,
        max_snapshot_bytes: 8 << 30,
        resources,
    })
    .await
    .unwrap();
    let service = Arc::new(ContainerService::new(
        Arc::clone(&artifacts.storage),
        artifacts.artifacts.clone(),
        Arc::clone(&artifacts.artifact_cache),
        provider,
    ));
    let metrics = Arc::new(
        crate::metrics::MetricsRegistry::new(
            &open_compute_core::config::MetricsConfig::default(),
            "test",
            "workerd",
        )
        .unwrap(),
    );
    let state = HttpState::for_test(
        crate::health::HealthCoordinator::new(),
        metrics,
        false,
        Some(open_compute_core::SecretString::new("admin-token")),
    )
    .with_v4_tokens(
        open_compute_core::SecretString::new("deployer-token"),
        open_compute_core::SecretString::new("read-token"),
    )
    .with_v4_instance_context(crate::cloudflare_v4::accounts::V4InstanceContext::new(
        service.instance_id(),
        0,
    ))
    .with_platform_storage(Arc::clone(&service.storage))
    .with_containers(Some(Arc::clone(&service)));
    Fixture {
        _artifacts: artifacts,
        _socket: directory,
        broker,
        shutdown,
        state,
        service,
        credentials,
        protocol,
    }
}
