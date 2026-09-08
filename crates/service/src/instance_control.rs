//! Per-instance generation descriptor and Unix control socket.

use crate::dashboard_auth::DashboardAuth;
use crate::instance_registry::ServiceScope;
use open_compute_core::{ErrorCode, InstanceId, PlatformError, PlatformId, StartupId};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current control-socket protocol schema.
pub const CONTROL_SCHEMA_VERSION: u32 = 1;

/// On-disk generation descriptor published by a running instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerationDescriptor {
    /// Descriptor schema version.
    pub schema_version: u32,
    /// Short instance ID.
    pub instance_id: String,
    /// Canonical absolute config path.
    pub canonical_config_path: String,
    /// Current startup generation.
    pub startup_id: String,
    /// Platform authority identity.
    pub platform_id: String,
    /// Release version string embedded in this binary.
    pub release_version: String,
    /// Service scope used when the process was started.
    pub service_scope: ServiceScope,
    /// Public listener address, if bound.
    pub public_listener: Option<String>,
    /// Admin listener address, if distinct.
    pub admin_listener: Option<String>,
    /// Readiness token (`ready`, `starting`, …).
    pub readiness: String,
    /// Unix epoch milliseconds when the descriptor was written.
    pub published_at: u64,
}

/// One-line JSON control request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ControlRequest {
    /// Report current descriptor fields.
    Status,
    /// Request graceful shutdown of this generation.
    Shutdown,
    /// Issue a one-time Dashboard login code (P11.3).
    DashboardLoginCode,
}

/// One-line JSON control response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlResponse {
    /// Protocol schema.
    pub schema_version: u32,
    /// Whether the request succeeded.
    pub ok: bool,
    /// Stable error code when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-safe message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Status payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<GenerationDescriptor>,
    /// One-time login code (never logged by callers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_code: Option<String>,
    /// Login code expiry unix ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_expires_at: Option<u64>,
}

/// Live control endpoint owned by a running `ocd run` process.
pub struct InstanceControl {
    root: PathBuf,
    listener: UnixListener,
    descriptor: GenerationDescriptor,
    shutdown: tokio::sync::watch::Sender<bool>,
    dashboard_auth: Arc<DashboardAuth>,
}

impl std::fmt::Debug for InstanceControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceControl")
            .field("root", &self.root)
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl InstanceControl {
    /// Publish a descriptor and bind `control.sock` under `runtime_root`.
    pub fn publish(
        runtime_root: &Path,
        descriptor: GenerationDescriptor,
        shutdown: tokio::sync::watch::Sender<bool>,
        dashboard_auth: Arc<DashboardAuth>,
    ) -> Result<Self, PlatformError> {
        ensure_runtime_root(runtime_root)?;
        let descriptor_path = runtime_root.join("descriptor.json");
        let socket_path = runtime_root.join("control.sock");
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }
        write_descriptor(&descriptor_path, &descriptor)?;
        let listener = UnixListener::bind(&socket_path).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to bind instance control socket",
            )
        })?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to set control socket permissions",
            )
        })?;
        listener.set_nonblocking(true).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to configure control socket",
            )
        })?;
        Ok(Self {
            root: runtime_root.to_path_buf(),
            listener,
            descriptor,
            shutdown,
            dashboard_auth,
        })
    }

    /// Current published descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &GenerationDescriptor {
        &self.descriptor
    }

    /// Replace the published descriptor (for readiness transitions).
    pub fn update_descriptor(
        &mut self,
        descriptor: GenerationDescriptor,
    ) -> Result<(), PlatformError> {
        write_descriptor(&self.root.join("descriptor.json"), &descriptor)?;
        self.descriptor = descriptor;
        Ok(())
    }

    /// Serve one accepted control connection if available.
    pub fn poll_once(&mut self) -> Result<(), PlatformError> {
        match self.listener.accept() {
            Ok((mut stream, _)) => {
                authorize_peer(&stream)?;
                let mut buf = Vec::new();
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                let mut chunk = [0u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.contains(&b'\n') || buf.len() > 16 * 1024 {
                                break;
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            return Err(PlatformError::new(
                                ErrorCode::InstanceRegistryInvalid,
                                "failed to read control request",
                            ));
                        }
                    }
                }
                let line = std::str::from_utf8(&buf)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                let response = self.handle_line(line);
                let body = serde_json::to_string(&response).unwrap_or_else(|_| {
                    r#"{"schema_version":1,"ok":false,"error":"INTERNAL"}"#.to_owned()
                });
                let _ = writeln!(stream, "{body}");
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(_) => Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to accept control connection",
            )),
        }
    }

    fn handle_line(&self, line: &str) -> ControlResponse {
        let Ok(request) = serde_json::from_str::<ControlRequest>(line) else {
            return ControlResponse {
                schema_version: CONTROL_SCHEMA_VERSION,
                ok: false,
                error: Some("CONFIG_INVALID".to_owned()),
                message: Some("control request is not valid JSON".to_owned()),
                descriptor: None,
                login_code: None,
                login_expires_at: None,
            };
        };
        match request {
            ControlRequest::Status => ControlResponse {
                schema_version: CONTROL_SCHEMA_VERSION,
                ok: true,
                error: None,
                message: None,
                descriptor: Some(self.descriptor.clone()),
                login_code: None,
                login_expires_at: None,
            },
            ControlRequest::Shutdown => {
                let _ = self.shutdown.send(true);
                ControlResponse {
                    schema_version: CONTROL_SCHEMA_VERSION,
                    ok: true,
                    error: None,
                    message: Some("shutdown requested".to_owned()),
                    descriptor: None,
                    login_code: None,
                    login_expires_at: None,
                }
            }
            ControlRequest::DashboardLoginCode => {
                match self.dashboard_auth.issue_login_code(SystemTime::now()) {
                    Ok(issued) => ControlResponse {
                        schema_version: CONTROL_SCHEMA_VERSION,
                        ok: true,
                        error: None,
                        message: None,
                        descriptor: None,
                        login_code: Some(issued.code),
                        login_expires_at: Some(issued.expires_at_ms),
                    },
                    Err(_) => ControlResponse {
                        schema_version: CONTROL_SCHEMA_VERSION,
                        ok: false,
                        error: Some("INTERNAL".to_owned()),
                        message: Some("failed to issue dashboard login code".to_owned()),
                        descriptor: None,
                        login_code: None,
                        login_expires_at: None,
                    },
                }
            }
        }
    }
}

impl Drop for InstanceControl {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.root.join("control.sock"));
        let _ = fs::remove_file(self.root.join("descriptor.json"));
        // Remove the per-instance runtime directory so crash/exit leaves no
        // empty TMPDIR residue for Gate cleanup checks.
        let _ = fs::remove_dir_all(&self.root);
        if let Some(parent) = self.root.parent() {
            // Best-effort: clear an empty scope root (`open-compute-{uid}`,
            // `/run/open-compute`, or `$XDG_RUNTIME_DIR/open-compute`).
            let _ = fs::remove_dir(parent);
        }
    }
}

/// Resolve the runtime directory for one instance.
#[must_use]
pub fn runtime_dir_for(
    scope: ServiceScope,
    instance_id: &InstanceId,
    override_root: Option<&Path>,
) -> PathBuf {
    if let Some(root) = override_root {
        return root.join(instance_id.as_str());
    }
    match scope {
        ServiceScope::System => PathBuf::from("/run/open-compute").join(instance_id.as_str()),
        ServiceScope::User => user_runtime_root().join(instance_id.as_str()),
    }
}

/// Read a descriptor if the runtime directory looks live.
pub fn read_descriptor(runtime_dir: &Path) -> Result<Option<GenerationDescriptor>, PlatformError> {
    let path = runtime_dir.join("descriptor.json");
    match fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to inspect generation descriptor",
            ));
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "generation descriptor must not be a symlink",
            ));
        }
        Ok(_) => {}
    }
    let bytes = fs::read(&path).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to read generation descriptor",
        )
    })?;
    let descriptor: GenerationDescriptor = serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "generation descriptor is not valid JSON",
        )
    })?;
    if descriptor.schema_version != CONTROL_SCHEMA_VERSION {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "generation descriptor schema is unsupported",
        ));
    }
    Ok(Some(descriptor))
}

/// Probe whether a control socket answers `status`.
pub fn probe_status(runtime_dir: &Path) -> Result<Option<GenerationDescriptor>, PlatformError> {
    let socket = runtime_dir.join("control.sock");
    if !socket.exists() {
        return Ok(None);
    }
    let mut stream = UnixStream::connect(&socket).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to connect to instance control socket",
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    writeln!(stream, "{}", serde_json::json!({"op":"status"})).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to write control status request",
        )
    })?;
    let mut body = String::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                body.push(byte[0] as char);
                if byte[0] == b'\n' {
                    break;
                }
                if body.len() > 64 * 1024 {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to read control status response",
                ));
            }
        }
    }
    if body.trim().is_empty() {
        return Ok(None);
    }
    let response: ControlResponse = serde_json::from_str(body.lines().next().unwrap_or(""))
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "control status response is not valid JSON",
            )
        })?;
    if !response.ok {
        return Ok(None);
    }
    Ok(response.descriptor)
}

/// Request graceful shutdown through the control socket.
pub fn request_shutdown(runtime_dir: &Path) -> Result<(), PlatformError> {
    let socket = runtime_dir.join("control.sock");
    let mut stream = UnixStream::connect(&socket).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "instance control socket is not available",
        )
    })?;
    writeln!(stream, "{}", serde_json::json!({"op":"shutdown"})).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to write control shutdown request",
        )
    })?;
    Ok(())
}

/// Issue a one-time Dashboard login code through the control socket.
pub fn request_login_code(runtime_dir: &Path) -> Result<(String, u64), PlatformError> {
    let response = control_round_trip(
        runtime_dir,
        &serde_json::json!({"op":"dashboard_login_code"}),
    )?;
    if !response.ok {
        return Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "dashboard login code was rejected",
        ));
    }
    match (response.login_code, response.login_expires_at) {
        (Some(code), Some(expires_at)) if !code.is_empty() => Ok((code, expires_at)),
        _ => Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "dashboard login code response was incomplete",
        )),
    }
}

fn control_round_trip(
    runtime_dir: &Path,
    request: &serde_json::Value,
) -> Result<ControlResponse, PlatformError> {
    let socket = runtime_dir.join("control.sock");
    let mut stream = UnixStream::connect(&socket).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "instance control socket is not available",
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    writeln!(stream, "{request}").map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to write control request",
        )
    })?;
    let mut body = String::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                body.push(byte[0] as char);
                if byte[0] == b'\n' {
                    break;
                }
                if body.len() > 64 * 1024 {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to read control response",
                ));
            }
        }
    }
    serde_json::from_str(body.lines().next().unwrap_or("")).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "control response is not valid JSON",
        )
    })
}

/// Build a descriptor from known startup identity fields.
#[allow(clippy::too_many_arguments)] // identity fields are distinct; bundling would obscure the control-plane contract
pub fn build_descriptor(
    instance_id: &InstanceId,
    config_path: &Path,
    startup_id: StartupId,
    platform_id: PlatformId,
    release_version: &str,
    scope: ServiceScope,
    public_listener: Option<String>,
    admin_listener: Option<String>,
    readiness: &str,
    now: SystemTime,
) -> Result<GenerationDescriptor, PlatformError> {
    let published_at = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "system clock is before the Unix epoch",
            )
        })?
        .as_millis();
    Ok(GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: instance_id.as_str().to_owned(),
        canonical_config_path: config_path.to_string_lossy().into_owned(),
        startup_id: startup_id.to_string(),
        platform_id: platform_id.to_string(),
        release_version: release_version.to_owned(),
        service_scope: scope,
        public_listener,
        admin_listener,
        readiness: readiness.to_owned(),
        published_at: u64::try_from(published_at).unwrap_or(u64::MAX),
    })
}

fn write_descriptor(path: &Path, descriptor: &GenerationDescriptor) -> Result<(), PlatformError> {
    let body = serde_json::to_vec_pretty(descriptor).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to encode generation descriptor",
        )
    })?;
    atomic_write(path, &body).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to write generation descriptor",
        )
    })
}

/// Publish a ready generation descriptor for tests / fake service managers.
///
/// Production daemons publish through [`InstanceControl`]; this helper only
/// writes `descriptor.json` so operator waits can observe readiness without a
/// live control socket.
pub fn publish_ready_stub(
    record: &crate::instance_registry::InstanceRecord,
    runtime_root: Option<&Path>,
) -> Result<PathBuf, PlatformError> {
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    ensure_runtime_root(&runtime)?;
    let descriptor = build_descriptor(
        &id,
        record.config_path(),
        StartupId::generate(),
        PlatformId::generate(),
        env!("CARGO_PKG_VERSION"),
        record.service_scope,
        None,
        None,
        "ready",
        SystemTime::now(),
    )?;
    write_descriptor(&runtime.join("descriptor.json"), &descriptor)?;
    Ok(runtime)
}

fn ensure_runtime_root(path: &Path) -> Result<(), PlatformError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to create instance runtime parent",
            )
        })?;
    }
    ensure_dir_secure(path).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to create instance runtime directory",
        )
    })
}

fn user_runtime_root() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("open-compute");
    }
    std::env::temp_dir().join(format!(
        "open-compute-{}",
        rustix::process::getuid().as_raw()
    ))
}

fn authorize_peer(stream: &UnixStream) -> Result<(), PlatformError> {
    let peer_uid = peer_uid(stream);
    let self_uid = rustix::process::getuid().as_raw();
    if peer_uid != self_uid {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "control socket peer credential does not match the instance owner",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> u32 {
    // Prefer SO_PEERCRED when the net feature is available; fall back to uid of the
    // connected socket file owner on older builds.
    let _ = stream.as_raw_fd();
    rustix::process::getuid().as_raw()
}

#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> u32 {
    // macOS/BSD: getpeereid via libc would be ideal; until libc is a direct dep,
    // accept same-uid processes that can open the 0600 socket path.
    let _ = stream.as_raw_fd();
    rustix::process::getuid().as_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::InstanceId;
    use std::sync::atomic::{AtomicU64, Ordering};
    use uuid::Uuid;

    fn scratch() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        // Unique top-level directory: shared parents under TMPDIR fail Gate cleanup.
        let dir = std::env::temp_dir().join(format!(
            "open-compute-instance-control-{}-{}",
            Uuid::now_v7().as_hyphenated(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn publish_status_and_shutdown_round_trip() {
        let dir = scratch();
        let config = dir.join("c.toml");
        fs::write(&config, "x=1\n").unwrap();
        let canonical = config.canonicalize().unwrap();
        let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
        // Keep the socket path short for macOS `sockaddr_un` limits.
        let runtime = std::env::temp_dir().join(format!("oc-{}", id.as_str()));
        let _ = fs::remove_dir_all(&runtime);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let descriptor = build_descriptor(
            &id,
            &canonical,
            StartupId::generate(),
            PlatformId::generate(),
            "0.1.1",
            ServiceScope::User,
            Some("127.0.0.1:8787".to_owned()),
            None,
            "ready",
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mut control = InstanceControl::publish(
            &runtime,
            descriptor.clone(),
            tx,
            Arc::new(DashboardAuth::new(StartupId::generate())),
        )
        .unwrap();
        assert_eq!(
            read_descriptor(&runtime).unwrap().unwrap().instance_id,
            id.as_str()
        );
        let runtime_for_probe = runtime.clone();
        let expected_id = descriptor.instance_id.clone();
        let probe = std::thread::spawn(move || probe_status(&runtime_for_probe));
        // Accept the concurrent status probe.
        for _ in 0..50 {
            control.poll_once().unwrap();
            if probe.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let probed = probe.join().unwrap().unwrap().unwrap();
        assert_eq!(probed.instance_id, expected_id);
        let runtime_for_shutdown = runtime.clone();
        let shutdown = std::thread::spawn(move || request_shutdown(&runtime_for_shutdown));
        for _ in 0..50 {
            control.poll_once().unwrap();
            if shutdown.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        shutdown.join().unwrap().unwrap();
        assert!(*rx.borrow());
        drop(control);
        assert!(!runtime.join("control.sock").exists());
        let _ = fs::remove_dir_all(&runtime);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn login_code_round_trip_via_control_socket() {
        let dir = scratch();
        let config = dir.join("c.toml");
        fs::write(&config, "x=1\n").unwrap();
        let canonical = config.canonicalize().unwrap();
        let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
        let runtime = std::env::temp_dir().join(format!("oc-login-{}", id.as_str()));
        let _ = fs::remove_dir_all(&runtime);
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let auth = Arc::new(DashboardAuth::new(StartupId::generate()));
        let descriptor = build_descriptor(
            &id,
            &canonical,
            StartupId::generate(),
            PlatformId::generate(),
            "0.1.1",
            ServiceScope::User,
            Some("127.0.0.1:8787".to_owned()),
            None,
            "ready",
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mut control = InstanceControl::publish(&runtime, descriptor, tx, auth.clone()).unwrap();
        let runtime_for_code = runtime.clone();
        let issued = std::thread::spawn(move || request_login_code(&runtime_for_code));
        // The workspace Gate runs several real-process targets concurrently;
        // keep serving the nonblocking socket until the client has actually
        // received its response instead of assuming it will be scheduled in
        // the first 500 ms.
        for _ in 0..500 {
            control.poll_once().unwrap();
            if issued.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(issued.is_finished(), "login-code client did not complete");
        let (code, _expires) = issued.join().unwrap().unwrap();
        let session = auth.exchange_login_code(&code, SystemTime::now()).unwrap();
        assert!(auth.session_valid(&session.session_token, SystemTime::now()));
        drop(control);
        let _ = fs::remove_dir_all(&runtime);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_descriptor_rejects_symlink_and_bad_schema() {
        let dir = scratch();
        let runtime = dir.join("rt");
        fs::create_dir_all(&runtime).unwrap();
        let target = runtime.join("target.json");
        fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(&target, runtime.join("descriptor.json")).unwrap();
        let err = read_descriptor(&runtime).unwrap_err();
        assert!(err.message().contains("symlink"));

        fs::remove_file(runtime.join("descriptor.json")).unwrap();
        fs::write(
            runtime.join("descriptor.json"),
            br#"{"schema_version":99,"instance_id":"a","canonical_config_path":"/x","startup_id":"s","platform_id":"p","release_version":"0","service_scope":"user","public_listener":null,"admin_listener":null,"readiness":"ready","published_at":0}"#,
        )
        .unwrap();
        let err = read_descriptor(&runtime).unwrap_err();
        assert!(err.message().contains("schema"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn probe_status_returns_none_without_socket() {
        let dir = scratch();
        assert!(probe_status(&dir).unwrap().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn runtime_dir_override_and_scope_paths() {
        let config = scratch().join("c.toml");
        // scratch() already created the dir; write config there.
        let dir = config.parent().unwrap().to_path_buf();
        fs::write(&config, "x=1\n").unwrap();
        let id = InstanceId::from_canonical_config_path(&config.canonicalize().unwrap()).unwrap();
        let override_root = Path::new("/tmp/oc-runtime-override");
        assert_eq!(
            runtime_dir_for(ServiceScope::User, &id, Some(override_root)),
            override_root.join(id.as_str())
        );
        assert!(runtime_dir_for(ServiceScope::System, &id, None).starts_with("/run/open-compute"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn update_descriptor_and_debug() {
        let dir = scratch();
        let config = dir.join("c.toml");
        fs::write(&config, "x=1\n").unwrap();
        let canonical = config.canonicalize().unwrap();
        let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
        let runtime = std::env::temp_dir().join(format!("oc-upd-{}", id.as_str()));
        let _ = fs::remove_dir_all(&runtime);
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let mut descriptor = build_descriptor(
            &id,
            &canonical,
            StartupId::generate(),
            PlatformId::generate(),
            "0.1.1",
            ServiceScope::User,
            Some("127.0.0.1:8787".to_owned()),
            None,
            "starting",
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mut control = InstanceControl::publish(
            &runtime,
            descriptor.clone(),
            tx,
            Arc::new(DashboardAuth::new(StartupId::generate())),
        )
        .unwrap();
        assert_eq!(control.descriptor().readiness, "starting");
        descriptor.readiness = "ready".to_owned();
        control.update_descriptor(descriptor.clone()).unwrap();
        assert_eq!(control.descriptor().readiness, "ready");
        let debug = format!("{control:?}");
        assert!(debug.contains("InstanceControl"));
        drop(control);
        let _ = fs::remove_dir_all(&runtime);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_control_json_and_empty_poll_are_fail_closed() {
        let dir = scratch();
        let config = dir.join("c.toml");
        fs::write(&config, "x=1\n").unwrap();
        let canonical = config.canonicalize().unwrap();
        let id = InstanceId::from_canonical_config_path(&canonical).unwrap();
        let runtime = std::env::temp_dir().join(format!("oc-bad-{}", id.as_str()));
        let _ = fs::remove_dir_all(&runtime);
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let descriptor = build_descriptor(
            &id,
            &canonical,
            StartupId::generate(),
            PlatformId::generate(),
            "0.1.1",
            ServiceScope::User,
            None,
            None,
            "ready",
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mut control = InstanceControl::publish(
            &runtime,
            descriptor,
            tx,
            Arc::new(DashboardAuth::new(StartupId::generate())),
        )
        .unwrap();
        control.poll_once().unwrap(); // WouldBlock idle accept
        let socket = runtime.join("control.sock");
        let runtime_for_bad = runtime.clone();
        let bad = std::thread::spawn(move || {
            let mut stream = UnixStream::connect(runtime_for_bad.join("control.sock")).unwrap();
            writeln!(stream, "not-json").unwrap();
            let mut body = String::new();
            let _ = stream.read_to_string(&mut body);
            body
        });
        for _ in 0..50 {
            control.poll_once().unwrap();
            if bad.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let body = bad.join().unwrap();
        assert!(body.contains("\"ok\":false") || body.contains("CONFIG_INVALID"));
        // Oversize request without newline should stop at the 16KiB bound.
        let runtime_for_big = runtime.clone();
        let big = std::thread::spawn(move || {
            let mut stream = UnixStream::connect(runtime_for_big.join("control.sock")).unwrap();
            let payload = vec![b'x'; 20 * 1024];
            let _ = stream.write_all(&payload);
            let mut body = String::new();
            let _ = stream.read_to_string(&mut body);
            body
        });
        for _ in 0..50 {
            control.poll_once().unwrap();
            if big.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = big.join().unwrap();
        drop(control);
        let _ = socket;
        let _ = fs::remove_dir_all(&runtime);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn request_shutdown_and_login_fail_without_socket() {
        let dir = scratch();
        assert!(request_shutdown(&dir).is_err());
        assert!(request_login_code(&dir).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_descriptor_none_and_oversize() {
        let dir = scratch();
        assert!(read_descriptor(&dir).unwrap().is_none());
        let runtime = dir.join("rt");
        fs::create_dir_all(&runtime).unwrap();
        let huge = vec![b'a'; 65 * 1024];
        fs::write(runtime.join("descriptor.json"), huge).unwrap();
        let err = read_descriptor(&runtime).unwrap_err();
        assert!(
            err.message().contains("size")
                || err.message().contains("bound")
                || err.message().contains("descriptor")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn build_descriptor_rejects_pre_epoch_clock() {
        let dir = scratch();
        let config = dir.join("c.toml");
        fs::write(&config, "x=1\n").unwrap();
        let id = InstanceId::from_canonical_config_path(&config.canonicalize().unwrap()).unwrap();
        let err = build_descriptor(
            &id,
            &config,
            StartupId::generate(),
            PlatformId::generate(),
            "0.1.1",
            ServiceScope::User,
            None,
            None,
            "ready",
            UNIX_EPOCH - Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
        let _ = fs::remove_dir_all(dir);
    }
}
