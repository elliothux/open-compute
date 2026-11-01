//! Deployment-time Python preparation, isolated from the instance runtime owner.

use super::*;
use crate::host_extension_broker::HostExtensionBroker;
use http_body_util::BodyExt as _;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{DurableObjectsConfig, RuntimeConfig, SecretBytes};
use open_compute_runtime::{
    CompileRequest, ConfigRole, ExternalServiceAddress, PlatformReleaseMeta,
    PythonPreparationOptions, PythonPreparationProcess, RuntimePackage, compile_static_config,
};
use open_compute_storage::PlatformStorage;
use open_compute_storage::worker_repository::{VersionState, WorkerRepository};
use open_compute_workers::python_artifact::{PreparedPythonIdentity, publish_prepared_python};
use std::os::unix::net::UnixStream as StdUnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::Instant;
use zeroize::Zeroizing;

const PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

pub(crate) struct PythonPreparation {
    pub(crate) storage: Arc<PlatformStorage>,
    pub(crate) artifacts: ArtifactStore,
    pub(crate) source: RuntimeSource,
    pub(crate) package: RuntimePackage,
    pub(crate) runtime_config: RuntimeConfig,
    pub(crate) durable_objects: DurableObjectsConfig,
    pub(crate) services: [ExternalServiceAddress; 3],
    pub(crate) broker: Arc<HostExtensionBroker>,
    pub(crate) serial: tokio::sync::Mutex<()>,
}

impl PythonPreparation {
    pub(super) async fn prepare(
        self: &Arc<Self>,
        candidate: ValidationCandidate,
        transport: WorkerdTransport,
    ) -> Result<(), PlatformError> {
        let version = WorkerRepository::new(self.storage.db()).get_version(
            candidate.instance_id,
            candidate.worker_id,
            candidate.version_id,
        )?;
        if !version
            .main_module
            .as_deref()
            .is_some_and(|main| main.ends_with(".py"))
        {
            return Ok(());
        }
        if version.state != VersionState::Validating {
            return Err(runtime_unavailable());
        }
        let preparation = self.clone();
        let (cancel, cancelled) = oneshot::channel();
        // The job retains the serial lock through cleanup even if the upload future
        // disappears. Cancellation prohibits publication; it never releases a live lease.
        let task =
            tokio::spawn(async move { preparation.run(candidate, transport, cancelled).await });
        let result = task.await.map_err(|_| runtime_unavailable())?;
        drop(cancel);
        result
    }

    async fn run(
        &self,
        candidate: ValidationCandidate,
        transport: WorkerdTransport,
        mut cancelled: oneshot::Receiver<()>,
    ) -> Result<(), PlatformError> {
        let _serial = tokio::select! {
            lock = self.serial.lock() => lock,
            _ = &mut cancelled => return Err(runtime_unavailable()),
        };
        ensure_not_cancelled(&mut cancelled)?;
        let deadline = Instant::now() + PREPARATION_TIMEOUT;
        let endpoint = transport.endpoint()?;
        let generation = endpoint.startup_id.ok_or_else(runtime_unavailable)?;
        let key = loader_key(
            candidate.instance_id,
            candidate.worker_id,
            candidate.version_id,
        );
        let expected = hex::encode(candidate.worker_code_sha256);
        let snapshot = tokio::time::timeout(
            remaining(deadline)?,
            self.source
                .resolve(&key, &expected, RuntimeScope::Validation),
        )
        .await
        .map_err(|_| runtime_unavailable())??;
        if let Some(prepared) = &snapshot.python_prepared_sha256 {
            // Restart/resume only restores the retained record, never re-runs imports.
            tokio::time::timeout(
                remaining(deadline)?,
                self.source.resolve_python_prepared(
                    &key,
                    &expected,
                    RuntimeScope::Validation,
                    prepared,
                ),
            )
            .await
            .map_err(|_| runtime_unavailable())??;
            return self.with_generation(&transport, generation, || {
                ensure_not_cancelled(&mut cancelled)
            });
        }
        let runtime_dir = self.storage.data_dir().runtime_dir();
        let redactor = open_compute_core::Redactor::new();
        let ingress_socket = endpoint.socket_path.with_file_name("prepare.sock");
        let runtime = self
            .package
            .verify(
                remaining(deadline)?,
                &redactor,
                &runtime_dir.join("python-compile.lease"),
            )
            .await?;
        let identity =
            PreparedPythonIdentity::from_snapshot(python_runtime_pin(&runtime), &snapshot)?;
        drop(snapshot);
        let compiled = compile_static_config(CompileRequest {
            role: ConfigRole::PythonPreparation,
            runtime: &runtime,
            lock_path: &self.package.lock_path(),
            assets_dir: &self.package.assets_dir(),
            runtime_data_dir: &runtime_dir,
            platform: &PlatformReleaseMeta {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            ingress_socket: &ingress_socket,
            durable_objects: self.durable_objects.clone(),
            container_provider: None,
            deadline: remaining(deadline)?,
            redactor: &redactor,
        })
        .await?;
        self.with_generation(&transport, generation, || {
            ensure_not_cancelled(&mut cancelled)
        })?;
        let (server, child) = StdUnixStream::pair().map_err(|_| runtime_unavailable())?;
        server
            .set_nonblocking(true)
            .map_err(|_| runtime_unavailable())?;
        let server = tokio::net::UnixStream::from_std(server).map_err(|_| runtime_unavailable())?;
        let broker = self.broker.clone();
        let mut brokers = JoinSet::new();
        brokers.spawn(async move { broker.serve_generation(server).await });
        let mut services = self.services.to_vec();
        services.push(ExternalServiceAddress::unix(
            "do-router",
            &endpoint.socket_path,
        )?);
        let process = PythonPreparationProcess::start(PythonPreparationOptions {
            runtime,
            compiled,
            startup_timeout: remaining(deadline)?.min(Duration::from_millis(
                self.runtime_config.startup_timeout_ms,
            )),
            lease_path: runtime_dir.join("python-prepare.lease"),
            external_services: services,
            host_extension_fd: Some(child.into()),
        })
        .await?;
        let response =
            Self::request_snapshot(&transport, process.ingress_socket(), &candidate, deadline)
                .await;
        // Always reap before either returning the request error or committing a snapshot.
        process
            .shutdown(
                Duration::from_millis(self.runtime_config.shutdown_grace_ms),
                Duration::from_millis(self.runtime_config.kill_timeout_ms),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(6), brokers.join_next())
            .await
            .map_err(|_| runtime_unavailable())?
            .ok_or_else(runtime_unavailable)?
            .map_err(|_| runtime_unavailable())??;
        let snapshot = response?;
        remaining(deadline)?;
        publish_prepared_python(
            &self.storage,
            &self.artifacts,
            &identity,
            &snapshot,
            open_compute_core::wall_time_ms(),
            |publish| {
                self.with_generation(&transport, generation, || {
                    ensure_not_cancelled(&mut cancelled)?;
                    remaining(deadline)?;
                    publish()
                })
            },
        )
        .await?;
        Ok(())
    }

    fn with_generation<T>(
        &self,
        transport: &WorkerdTransport,
        generation: open_compute_core::StartupId,
        operation: impl FnOnce() -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        if transport.current_generation() != Some(generation) {
            return Err(runtime_unavailable());
        }
        operation()
    }

    async fn request_snapshot(
        transport: &WorkerdTransport,
        socket: &Path,
        candidate: &ValidationCandidate,
        deadline: Instant,
    ) -> Result<SecretBytes, PlatformError> {
        let request = Request::builder()
            .method(Method::POST)
            .uri("http://runtime/internal/prepare-python")
            .header(
                "x-open-compute-instance-id",
                candidate.instance_id.to_string(),
            )
            .header(
                "x-open-compute-loader-key",
                loader_key(
                    candidate.instance_id,
                    candidate.worker_id,
                    candidate.version_id,
                ),
            )
            .header(
                "x-open-compute-worker-code-sha256",
                hex::encode(candidate.worker_code_sha256),
            )
            .header("x-open-compute-route-generation", "0")
            .body(Body::empty())
            .map_err(|_| runtime_unavailable())?;
        tokio::time::timeout(remaining(deadline)?, async {
            let response = transport
                .request(socket, request)
                .await
                .map_err(|_| runtime_unavailable())?;
            collect_snapshot(response.map(Body::new)).await
        })
        .await
        .map_err(|_| runtime_unavailable())?
    }
}

fn remaining(deadline: Instant) -> Result<Duration, PlatformError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(runtime_unavailable())
    } else {
        Ok(remaining)
    }
}

fn ensure_not_cancelled(cancelled: &mut oneshot::Receiver<()>) -> Result<(), PlatformError> {
    if matches!(
        cancelled.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ) {
        Ok(())
    } else {
        Err(runtime_unavailable())
    }
}

async fn collect_snapshot(response: Response) -> Result<SecretBytes, PlatformError> {
    if response.status() != StatusCode::OK {
        let code = match (
            response.status(),
            response
                .headers()
                .get(ERROR_HEADER)
                .and_then(|value| value.to_str().ok()),
        ) {
            (StatusCode::UNPROCESSABLE_ENTITY, Some("BUNDLE_RUNTIME_INVALID")) => {
                ErrorCode::BundleRuntimeInvalid
            }
            (StatusCode::CONFLICT, Some("VERSION_NOT_READY")) => ErrorCode::VersionNotReady,
            (StatusCode::SERVICE_UNAVAILABLE, Some("ARTIFACT_UNAVAILABLE")) => {
                ErrorCode::ArtifactUnavailable
            }
            _ => ErrorCode::RuntimeUnavailable,
        };
        return Err(PlatformError::new(code, "Python preparation failed"));
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
        || response.headers().contains_key(header::CONTENT_ENCODING)
        || response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            != Some("no-store")
    {
        return Err(runtime_unavailable());
    }
    let length = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(runtime_unavailable)?;
    let size = length
        .parse::<usize>()
        .ok()
        .filter(|size| (16..=MAX_SNAPSHOT_BYTES).contains(size) && size.to_string() == length)
        .ok_or_else(runtime_unavailable)?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(size)
        .map_err(|_| runtime_unavailable())?;
    let mut buffer = Zeroizing::new(buffer);
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let data = frame
            .map_err(|_| runtime_unavailable())?
            .into_data()
            .map_err(|_| runtime_unavailable())?;
        if data.len() > size.saturating_sub(buffer.len()) {
            return Err(runtime_unavailable());
        }
        buffer.extend_from_slice(&data);
    }
    if buffer.len() != size {
        return Err(runtime_unavailable());
    }
    Ok(SecretBytes::new(std::mem::take(&mut *buffer)))
}

#[cfg(test)]
#[path = "python_preparation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "python_preparation_native_tests.rs"]
mod native_tests;
