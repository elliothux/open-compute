//! One scoped daemon lifecycle authority shared by its local control surfaces.

use crate::cloudflare_v4::V4Role;
use crate::instance_registry::InstanceRecord;
use open_compute_artifacts::{ArtifactCache, CacheCleanReport};
use open_compute_core::{ErrorCode, InstanceId, PlatformError, SecretString};
use open_compute_storage::PlatformStorage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Clone)]
pub(crate) struct DaemonApi {
    commands: mpsc::Sender<DaemonCommand>,
    instances: Arc<RwLock<HashMap<InstanceId, RegisteredInstance>>>,
    admin_token: Arc<SecretString>,
    gateway: Arc<RwLock<Option<Arc<crate::gateway_control::GatewayControl>>>>,
}

pub(crate) struct RegisteredTokens {
    pub(crate) instance_id: InstanceId,
    pub(crate) deployer: SecretString,
    pub(crate) read_only: SecretString,
}

struct RegisteredInstance {
    view: InstanceView,
    tokens: RegisteredTokens,
    public_base_domain: Option<String>,
    cache: Option<LiveCache>,
}

struct LiveCache {
    cache: Weak<ArtifactCache>,
    storage: Weak<PlatformStorage>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct InstanceView {
    pub(crate) instance_id: String,
    pub(crate) name: Option<String>,
    pub(crate) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum LifecycleAction {
    Start,
    Stop,
}

pub(crate) struct DaemonCommand {
    pub(crate) request: ControlRequest,
    pub(crate) reply: oneshot::Sender<Result<Option<CacheCleanReport>, PlatformError>>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ControlRequest {
    List,
    CaddyStatus,
    CaddyReload,
    CaddyValidate,
    Start {
        instance_id: InstanceId,
    },
    Stop {
        instance_id: InstanceId,
    },
    Add {
        config_path: PathBuf,
    },
    Create {
        config_path: PathBuf,
        data_dir: PathBuf,
        name: Option<open_compute_core::InstanceName>,
        autostart: bool,
        start: bool,
    },
    Remove {
        instance_id: InstanceId,
    },
    CleanCache {
        instance_id: InstanceId,
        dry_run: bool,
    },
    CleanGlobalCache {
        dry_run: bool,
    },
}

#[derive(Deserialize, Serialize)]
pub(crate) struct ControlResponse {
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) instances: Option<Vec<InstanceView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gateway_status: Option<crate::gateway_control::GatewayStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_report: Option<CacheCleanReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

pub(crate) struct DaemonSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl DaemonApi {
    pub(crate) fn channel(
        records: &[InstanceRecord],
        credentials: Vec<RegisteredTokens>,
        admin_token: SecretString,
    ) -> Result<(Self, mpsc::Receiver<DaemonCommand>), PlatformError> {
        let mut credentials = credentials
            .into_iter()
            .map(|entry| (entry.instance_id, entry))
            .collect::<HashMap<_, _>>();
        if credentials.len() != records.len() {
            return Err(unavailable());
        }
        let mut instances = HashMap::with_capacity(records.len());
        for record in records {
            let id = record.instance_id()?;
            let tokens = credentials.remove(&id).ok_or_else(unavailable)?;
            instances.insert(
                id,
                RegisteredInstance {
                    view: InstanceView {
                        instance_id: record.instance_id.clone(),
                        name: record.name.clone(),
                        state: "stopped".to_owned(),
                        error: None,
                    },
                    tokens,
                    public_base_domain: record.public_base_domain.clone(),
                    cache: None,
                },
            );
        }
        if !credentials.is_empty() || instances.len() != records.len() {
            return Err(unavailable());
        }
        let (commands, receiver) = mpsc::channel(32);
        Ok((
            Self {
                commands,
                instances: Arc::new(RwLock::new(instances)),
                admin_token: Arc::new(admin_token),
                gateway: Arc::new(RwLock::new(None)),
            },
            receiver,
        ))
    }

    pub(crate) fn authorized(&self, bearer: Option<&str>) -> bool {
        crate::auth::bearer_matches(bearer, &self.admin_token)
    }

    pub(crate) fn set_gateway(
        &self,
        gateway: Arc<crate::gateway_control::GatewayControl>,
    ) -> Result<(), PlatformError> {
        *self.gateway.write().map_err(|_| unavailable())? = Some(gateway);
        Ok(())
    }

    fn caddy(
        &self,
        request: &ControlRequest,
    ) -> Result<crate::gateway_control::GatewayStatus, PlatformError> {
        let gateway = self.gateway.read().map_err(|_| unavailable())?;
        let control = gateway.as_ref().ok_or_else(|| {
            PlatformError::new(ErrorCode::ConfigInvalid, "shared Gateway is not configured")
        })?;
        match request {
            ControlRequest::CaddyStatus => Ok(control.status()),
            ControlRequest::CaddyReload => control.reload(),
            ControlRequest::CaddyValidate => control.validate(),
            _ => Err(unavailable()),
        }
    }

    pub(crate) fn list(&self) -> Result<Vec<InstanceView>, PlatformError> {
        let mut views = self
            .instances
            .read()
            .map_err(|_| unavailable())?
            .values()
            .map(|entry| entry.view.clone())
            .collect::<Vec<_>>();
        views.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(views)
    }

    pub(crate) fn visible_for_bearer(
        &self,
        bearer: Option<&str>,
    ) -> Result<Option<(Vec<InstanceView>, V4Role)>, PlatformError> {
        if self.authorized(bearer) {
            return self.list().map(|views| Some((views, V4Role::Admin)));
        }
        let instances = self.instances.read().map_err(|_| unavailable())?;
        for entry in instances.values() {
            let role = if crate::auth::bearer_matches(bearer, &entry.tokens.deployer) {
                Some(V4Role::Deployer)
            } else if crate::auth::bearer_matches(bearer, &entry.tokens.read_only) {
                Some(V4Role::ReadOnly)
            } else {
                None
            };
            if let Some(role) = role {
                return Ok(Some((vec![entry.view.clone()], role)));
            }
        }
        Ok(None)
    }

    pub(crate) fn matches_runtime(
        &self,
        id: &InstanceId,
        admin: &SecretString,
        deployer: &SecretString,
        read_only: &SecretString,
        public_base_domain: Option<&str>,
    ) -> Result<bool, PlatformError> {
        let instances = self.instances.read().map_err(|_| unavailable())?;
        let Some(entry) = instances.get(id) else {
            return Ok(false);
        };
        Ok(self.admin_token.expose() == admin.expose()
            && entry.tokens.deployer.expose() == deployer.expose()
            && entry.tokens.read_only.expose() == read_only.expose()
            && entry.public_base_domain.as_deref() == public_base_domain)
    }

    pub(crate) fn mark(
        &self,
        id: &InstanceId,
        state: &'static str,
        error: Option<ErrorCode>,
    ) -> Result<(), PlatformError> {
        let mut instances = self.instances.write().map_err(|_| unavailable())?;
        let instance = instances.get_mut(id).ok_or_else(|| {
            PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
        })?;
        instance.view.state = state.to_owned();
        instance.view.error = error.map(|value| value.as_str().to_owned());
        Ok(())
    }

    pub(crate) fn insert(
        &self,
        record: &InstanceRecord,
        credentials: RegisteredTokens,
    ) -> Result<(), PlatformError> {
        let mut instances = self.instances.write().map_err(|_| unavailable())?;
        let id = record.instance_id()?;
        if instances.contains_key(&id) || credentials.instance_id != id {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance is already registered",
            ));
        }
        instances.insert(
            id,
            RegisteredInstance {
                view: InstanceView {
                    instance_id: record.instance_id.clone(),
                    name: record.name.clone(),
                    state: "stopped".to_owned(),
                    error: None,
                },
                tokens: credentials,
                public_base_domain: record.public_base_domain.clone(),
                cache: None,
            },
        );
        Ok(())
    }

    pub(crate) fn refresh_stopped(
        &self,
        record: &InstanceRecord,
        credentials: RegisteredTokens,
    ) -> Result<(), PlatformError> {
        let id = record.instance_id()?;
        if credentials.instance_id != id {
            return Err(unavailable());
        }
        let mut instances = self.instances.write().map_err(|_| unavailable())?;
        let entry = instances.get_mut(&id).ok_or_else(unavailable)?;
        if !matches!(entry.view.state.as_str(), "stopped" | "failed") {
            return Err(unavailable());
        }
        entry.view.name = record.name.clone();
        entry.tokens = credentials;
        entry.public_base_domain = record.public_base_domain.clone();
        Ok(())
    }

    pub(crate) fn remove(&self, id: &InstanceId) -> Result<(), PlatformError> {
        self.instances
            .write()
            .map_err(|_| unavailable())?
            .remove(id)
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
            })?;
        Ok(())
    }

    pub(crate) fn register_cache(
        &self,
        id: InstanceId,
        cache: &Arc<ArtifactCache>,
        storage: &Arc<PlatformStorage>,
    ) -> Result<(), PlatformError> {
        self.instances
            .write()
            .map_err(|_| unavailable())?
            .get_mut(&id)
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
            })?
            .cache = Some(LiveCache {
            cache: Arc::downgrade(cache),
            storage: Arc::downgrade(storage),
        });
        Ok(())
    }

    pub(crate) async fn clean_cache(
        &self,
        id: &InstanceId,
        dry_run: bool,
    ) -> Result<CacheCleanReport, PlatformError> {
        let (_storage, cache) = {
            let instances = self.instances.read().map_err(|_| unavailable())?;
            let live = instances
                .get(id)
                .ok_or_else(|| {
                    PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
                })?
                .cache
                .as_ref()
                .ok_or_else(unavailable)?;
            (
                live.storage.upgrade().ok_or_else(unavailable)?,
                live.cache.upgrade().ok_or_else(unavailable)?,
            )
        };
        cache.clean(dry_run).await
    }

    pub(crate) async fn request(
        &self,
        action: LifecycleAction,
        instance_id: InstanceId,
    ) -> Result<(), PlatformError> {
        let request = match action {
            LifecycleAction::Start => ControlRequest::Start { instance_id },
            LifecycleAction::Stop => ControlRequest::Stop { instance_id },
        };
        self.command(request).await
    }

    pub(crate) async fn command(&self, request: ControlRequest) -> Result<(), PlatformError> {
        self.command_with_report(request).await.map(|_| ())
    }

    async fn command_with_report(
        &self,
        request: ControlRequest,
    ) -> Result<Option<CacheCleanReport>, PlatformError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(DaemonCommand { request, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "daemon lifecycle authority is unavailable",
    )
}

impl DaemonSocket {
    pub(crate) fn bind(root: &Path) -> Result<Self, PlatformError> {
        let run = root.join("run");
        open_compute_storage::ensure_dir_secure(&run)?;
        let path = run.join("control.sock");
        if !crate::instance_control::unix_socket_path_is_valid(&path) {
            return Err(invalid("daemon control socket path is invalid"));
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.file_type().is_socket()
                    || meta.uid() != rustix::process::getuid().as_raw()
                    || meta.permissions().mode() & 0o077 != 0
                    || UnixStream::connect(&path).is_ok()
                {
                    return Err(invalid("daemon control socket is already owned"));
                }
                fs::remove_file(&path)
                    .map_err(|_| invalid("failed to remove stale daemon socket"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid("failed to inspect daemon control socket")),
        }
        let listener = UnixListener::bind(&path)
            .map_err(|_| invalid("failed to bind daemon control socket"))?;
        if fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).is_err() {
            let _ = fs::remove_file(&path);
            return Err(invalid("failed to secure daemon control socket"));
        }
        Ok(Self { listener, path })
    }

    pub(crate) async fn serve(
        self,
        api: DaemonApi,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        loop {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted.map_err(|_| invalid("failed to accept daemon control connection"))?;
                    if let Err(error) = handle_connection(stream, &api).await {
                        tracing::warn!(code = error.code().as_str(), "daemon control request rejected");
                    }
                }
            }
        }
    }
}

impl Drop for DaemonSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    api: &DaemonApi,
) -> Result<(), PlatformError> {
    let peer = stream
        .peer_cred()
        .map_err(|_| invalid("daemon control peer credentials are unavailable"))?;
    let uid = rustix::process::getuid().as_raw();
    if peer.uid() != uid && peer.uid() != 0 {
        return Err(invalid("daemon control peer is not authorized"));
    }
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let byte = stream
                .read_u8()
                .await
                .map_err(|_| invalid("failed to read daemon control request"))?;
            if byte == b'\n' {
                return Ok::<(), PlatformError>(());
            }
            if bytes.len() >= 16 * 1024 {
                return Err(invalid("daemon control request is too large"));
            }
            bytes.push(byte);
        }
    })
    .await
    .map_err(|_| invalid("daemon control request timed out"))??;
    let response = match serde_json::from_slice::<ControlRequest>(&bytes) {
        Ok(ControlRequest::List) => match api.list() {
            Ok(instances) => ControlResponse {
                ok: true,
                instances: Some(instances),
                gateway_status: None,
                cache_report: None,
                error: None,
            },
            Err(error) => error_response(&error),
        },
        Ok(
            request @ (ControlRequest::CaddyStatus
            | ControlRequest::CaddyReload
            | ControlRequest::CaddyValidate),
        ) => match api.caddy(&request) {
            Ok(status) => ControlResponse {
                ok: true,
                instances: None,
                gateway_status: Some(status),
                cache_report: None,
                error: None,
            },
            Err(error) => error_response(&error),
        },
        Ok(request) => operation_response(api.command_with_report(request).await),
        Err(_) => error_response(&invalid("daemon control request is invalid")),
    };
    let mut body = serde_json::to_vec(&response).map_err(|_| unavailable())?;
    body.push(b'\n');
    stream
        .write_all(&body)
        .await
        .map_err(|_| invalid("failed to write daemon control response"))
}

fn operation_response(result: Result<Option<CacheCleanReport>, PlatformError>) -> ControlResponse {
    match result {
        Ok(report) => ControlResponse {
            ok: true,
            instances: None,
            gateway_status: None,
            cache_report: report,
            error: None,
        },
        Err(error) => error_response(&error),
    }
}

fn error_response(error: &PlatformError) -> ControlResponse {
    ControlResponse {
        ok: false,
        instances: None,
        gateway_status: None,
        cache_report: None,
        error: Some(error.code().as_str().to_owned()),
    }
}

pub(crate) fn exchange(
    root: &Path,
    request: &ControlRequest,
) -> Result<ControlResponse, PlatformError> {
    let run = root.join("run");
    let path = run.join("control.sock");
    if !crate::instance_control::unix_socket_path_is_valid(&path) {
        return Err(invalid("daemon control socket path is invalid"));
    }
    let root_meta = fs::symlink_metadata(root).map_err(|_| invalid("OCD root is unavailable"))?;
    let run_meta =
        fs::symlink_metadata(&run).map_err(|_| invalid("OCD run directory is unavailable"))?;
    let socket_meta =
        fs::symlink_metadata(&path).map_err(|_| invalid("daemon control socket is unavailable"))?;
    let uid = rustix::process::getuid().as_raw();
    if !root_meta.is_dir()
        || !run_meta.is_dir()
        || !socket_meta.file_type().is_socket()
        || root_meta.uid() != run_meta.uid()
        || root_meta.uid() != socket_meta.uid()
        || (uid != 0 && uid != root_meta.uid())
        || run_meta.permissions().mode() & 0o077 != 0
        || socket_meta.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("daemon control socket owner or mode is invalid"));
    }
    let mut stream = UnixStream::connect(&path).map_err(|_| invalid("daemon is not running"))?;
    let timeout = match request {
        ControlRequest::CleanCache { .. } | ControlRequest::CleanGlobalCache { .. } => {
            Duration::from_secs(300)
        }
        ControlRequest::Add { .. }
        | ControlRequest::Create { .. }
        | ControlRequest::Remove { .. } => Duration::from_secs(60),
        ControlRequest::CaddyReload | ControlRequest::CaddyValidate => Duration::from_secs(30),
        _ => Duration::from_secs(3),
    };
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| invalid("failed to set daemon control timeout"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| invalid("failed to set daemon control timeout"))?;
    let body = serde_json::to_string(request)
        .map_err(|_| invalid("failed to encode daemon control request"))?;
    writeln!(stream, "{body}").map_err(|_| invalid("failed to send daemon control request"))?;
    let mut bytes = Vec::new();
    stream
        .take(256 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("failed to read daemon control response"))?;
    serde_json::from_slice(&bytes).map_err(|_| invalid("daemon control response is invalid"))
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::InstanceRegistryInvalid, message)
}

#[cfg(test)]
#[path = "daemon_control_tests.rs"]
mod tests;
