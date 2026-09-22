//! Serialized managed Caddy configuration and secret-free runtime status.

use open_compute_core::{ErrorCode, PlatformError, PublicGatewayConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

const MAX_ADMIN_RESPONSE: u64 = 8 * 1024 * 1024;

/// Operator-safe state for the one managed Caddy child.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GatewayStatus {
    /// Status schema.
    pub(crate) schema_version: u32,
    /// Current child PID, absent while stopped or restarting.
    pub(crate) child_pid: Option<i32>,
    /// Whether the current child passed the platform TLS probe.
    pub(crate) tls_ready: bool,
    /// SHA-256 of the last successfully loaded complete JSON config.
    pub(crate) config_sha256: Option<String>,
    /// Result token for the most recent explicit reload.
    pub(crate) last_reload: String,
    /// Stable, secret-free error category for the most recent reload.
    pub(crate) last_error: Option<String>,
    /// Result of the most recent public DNS drift verification.
    pub(crate) dns: String,
}

#[derive(Debug)]
struct ReloadState {
    config_sha256: Option<String>,
    last_reload: &'static str,
    last_error: Option<&'static str>,
    dns: &'static str,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotMeta {
    schema_version: u32,
    intent_sha256: String,
    payload_sha256: String,
}

/// Synchronous control owner called only through the local operator socket.
pub(crate) struct GatewayControl {
    config: PublicGatewayConfig,
    gateway_dir: PathBuf,
    upstream_path: PathBuf,
    provider_path: PathBuf,
    child_pid: std::sync::Arc<AtomicI32>,
    qualified_pid: std::sync::Arc<AtomicI32>,
    reload: Mutex<ReloadState>,
}

impl GatewayControl {
    /// Construct the one gateway control owner.
    pub(crate) fn new(
        config: PublicGatewayConfig,
        gateway_dir: PathBuf,
        upstream_path: PathBuf,
        provider_path: PathBuf,
        child_pid: std::sync::Arc<AtomicI32>,
        qualified_pid: std::sync::Arc<AtomicI32>,
    ) -> Self {
        let digest = snapshot_digest(&gateway_dir.join("config-state/current.json"));
        Self {
            config,
            gateway_dir,
            upstream_path,
            provider_path,
            child_pid,
            qualified_pid,
            reload: Mutex::new(ReloadState {
                config_sha256: digest,
                last_reload: "never",
                last_error: None,
                dns: "pending",
            }),
        }
    }

    /// Return current secret-free process/config state.
    pub(crate) fn status(&self) -> GatewayStatus {
        let pid = self.child_pid.load(Ordering::Acquire);
        let state = self
            .reload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        GatewayStatus {
            schema_version: 1,
            child_pid: (pid > 0).then_some(pid),
            tls_ready: pid > 0 && self.qualified_pid.load(Ordering::Acquire) == pid,
            config_sha256: state.config_sha256.clone(),
            last_reload: state.last_reload.to_owned(),
            last_error: state.last_error.map(str::to_owned),
            dns: state.dns.to_owned(),
        }
    }

    /// Record one bounded, read-only public DNS drift check.
    pub(crate) fn record_dns_result(&self, passed: bool) {
        let mut state = self
            .reload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.dns = if passed { "ok" } else { "failed" };
    }

    /// Adapt, validate, load, and commit one complete configuration.
    pub(crate) fn reload(&self) -> Result<GatewayStatus, PlatformError> {
        let mut state = self
            .reload
            .lock()
            .map_err(|_| control_error("gateway reload state is unavailable"))?;
        state.last_reload = "failed";
        state.last_error = Some("candidate_invalid");
        let candidate = self
            .gateway_dir
            .join("config-state")
            .join(format!("candidate-{}", uuid::Uuid::now_v7()));
        open_compute_storage::ensure_dir_secure(&candidate)?;
        let result = self.reload_candidate(&candidate, &mut state);
        match result {
            Ok(digest) => {
                let _ = fs::remove_dir_all(&candidate);
                state.config_sha256 = Some(digest);
                state.last_reload = "ok";
                state.last_error = None;
                drop(state);
                Ok(self.status())
            }
            Err(error) => Err(error),
        }
    }

    /// Adapt and validate one complete candidate without changing live config.
    pub(crate) fn validate(&self) -> Result<GatewayStatus, PlatformError> {
        let guard = self
            .reload
            .lock()
            .map_err(|_| control_error("gateway reload state is unavailable"))?;
        let candidate = self
            .gateway_dir
            .join("config-state")
            .join(format!("validate-{}", uuid::Uuid::now_v7()));
        open_compute_storage::ensure_dir_secure(&candidate)?;
        let result = self.adapt_candidate(&candidate);
        if result.is_ok() {
            let _ = fs::remove_dir_all(&candidate);
        }
        drop(guard);
        result?;
        Ok(self.status())
    }

    fn reload_candidate(
        &self,
        candidate: &Path,
        state: &mut ReloadState,
    ) -> Result<String, PlatformError> {
        state.last_error = Some("adapt_failed");
        let adapted = self.adapt_candidate(candidate)?;
        state.last_error = Some("load_failed");
        open_compute_storage::atomic_write(&candidate.join("candidate.json"), &adapted)?;
        admin_request(
            &self.gateway_dir.join("run/admin.sock"),
            "POST",
            "/load",
            "application/json",
            &adapted,
        )?;
        state.last_error = Some("snapshot_failed");
        let state_dir = self.gateway_dir.join("config-state");
        let current = state_dir.join("current.json");
        if current.exists() {
            let bytes = fs::read(&current)
                .map_err(|_| control_error("failed to preserve prior Caddy snapshot"))?;
            open_compute_storage::atomic_write(&state_dir.join("previous.json"), &bytes)?;
        }
        open_compute_storage::atomic_write(&current, &adapted)?;
        let meta = SnapshotMeta {
            schema_version: 1,
            intent_sha256: intent_digest(&self.config)?,
            payload_sha256: open_compute_runtime::embedded_payload_sha256().to_owned(),
        };
        let meta = serde_json::to_vec(&meta)
            .map_err(|_| control_error("failed to encode Caddy snapshot metadata"))?;
        open_compute_storage::atomic_write(&state_dir.join("current.meta.json"), &meta)?;
        crate::gateway_caddyfile::write_managed(
            &self.config,
            &self.gateway_dir,
            &self.upstream_path,
            &self.provider_path,
        )?;
        Ok(hex::encode(Sha256::digest(&adapted)))
    }

    fn adapt_candidate(&self, candidate: &Path) -> Result<Vec<u8>, PlatformError> {
        let managed_path = candidate.join("managed.caddyfile");
        let (managed, entrypoint) = crate::gateway_caddyfile::render_candidate(
            &self.config,
            &self.gateway_dir,
            &managed_path,
            &self.upstream_path,
            &self.provider_path,
        )?;
        open_compute_storage::atomic_write(&managed_path, managed.as_bytes())?;
        open_compute_storage::atomic_write(&candidate.join("Caddyfile"), entrypoint.as_bytes())?;
        let source = fs::read(candidate.join("Caddyfile"))
            .map_err(|_| control_error("failed to read Caddy reload candidate"))?;
        let adapted_response = admin_request(
            &self.gateway_dir.join("run/admin.sock"),
            "POST",
            "/adapt",
            "text/caddyfile",
            &source,
        )?;
        let adapted_response: serde_json::Value = serde_json::from_slice(&adapted_response)
            .map_err(|_| control_error("Caddy adapter returned invalid JSON"))?;
        let adapted = serde_json::to_vec(
            adapted_response
                .get("result")
                .ok_or_else(|| control_error("Caddy adapter omitted its result"))?,
        )
        .map_err(|_| control_error("failed to encode adapted Caddy configuration"))?;
        Ok(adapted)
    }
}

/// Return the confirmed snapshot when it matches current platform intent and pin.
pub(crate) fn confirmed_snapshot(
    config: &PublicGatewayConfig,
    gateway_dir: &Path,
) -> Option<PathBuf> {
    let bytes = fs::read(gateway_dir.join("config-state/current.meta.json")).ok()?;
    let meta: SnapshotMeta = serde_json::from_slice(&bytes).ok()?;
    if meta.schema_version != 1
        || meta.payload_sha256 != open_compute_runtime::embedded_payload_sha256()
        || meta.intent_sha256 != intent_digest(config).ok()?
    {
        return None;
    }
    let snapshot = gateway_dir.join("config-state/current.json");
    serde_json::from_slice::<serde_json::Value>(&fs::read(&snapshot).ok()?).ok()?;
    Some(snapshot)
}

fn admin_request(
    socket: &Path,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> Result<Vec<u8>, PlatformError> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|_| control_error("managed Caddy admin socket is unavailable"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .and_then(|()| stream.write_all(body))
    .map_err(|_| control_error("failed to write managed Caddy request"))?;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|_| control_error("failed to read managed Caddy response"))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        if response.len() as u64 > MAX_ADMIN_RESPONSE {
            return Err(control_error("managed Caddy response exceeded its bound"));
        }
        if response_complete(&response)? {
            break;
        }
    }
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| control_error("managed Caddy returned an invalid HTTP response"))?;
    let headers = std::str::from_utf8(&response[..split])
        .map_err(|_| control_error("managed Caddy returned invalid HTTP headers"))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| control_error("managed Caddy returned an invalid HTTP status"))?;
    if !(200..300).contains(&status) {
        return Err(control_error(
            "managed Caddy rejected the complete configuration",
        ));
    }
    Ok(response[split + 4..].to_vec())
}

fn response_complete(response: &[u8]) -> Result<bool, PlatformError> {
    let Some(split) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(false);
    };
    let headers = std::str::from_utf8(&response[..split])
        .map_err(|_| control_error("managed Caddy returned invalid HTTP headers"))?;
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or_else(|| control_error("managed Caddy response omitted Content-Length"))?;
    Ok(response.len().saturating_sub(split + 4) >= length)
}

fn snapshot_digest(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(hex::encode(Sha256::digest(bytes)))
}

fn intent_digest(config: &PublicGatewayConfig) -> Result<String, PlatformError> {
    let bytes = serde_json::to_vec(config)
        .map_err(|_| control_error("failed to encode Caddy platform intent"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn control_error(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::PublicGatewayConfig;
    use std::net::Ipv4Addr;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;

    fn config() -> PublicGatewayConfig {
        PublicGatewayConfig {
            base_domain: "compute.example.com".to_owned(),
            ingress_ipv4: vec![Ipv4Addr::new(203, 0, 113, 10)],
            ingress_ipv6: Vec::new(),
            https_listen: "127.0.0.1:8443".parse().unwrap(),
            challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
            proxy_protocol_from: Vec::new(),
            caddy: Vec::new(),
        }
    }

    fn control(root: &Path) -> GatewayControl {
        for directory in ["run", "storage", "config-state"] {
            open_compute_storage::ensure_dir_secure(&root.join(directory)).unwrap();
        }
        GatewayControl::new(
            config(),
            root.to_owned(),
            root.join("run/gw.sock"),
            root.join("run/dns.sock"),
            Arc::new(AtomicI32::new(41)),
            Arc::new(AtomicI32::new(41)),
        )
    }

    fn serve_admin(socket: &Path, responses: Vec<&'static [u8]>) -> std::thread::JoinHandle<()> {
        let _ = fs::remove_file(socket);
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    let complete = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .and_then(|split| {
                            let headers = std::str::from_utf8(&request[..split]).ok()?;
                            let length = headers.lines().find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })?;
                            Some(request.len() >= split + 4 + length)
                        })
                        .unwrap_or(false);
                    if read == 0 || complete {
                        break;
                    }
                }
                stream.write_all(response).unwrap();
            }
        })
    }

    #[test]
    fn bounded_admin_response_waits_for_the_declared_body() {
        assert!(!response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").unwrap());
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap());
        assert!(response_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap());
        assert!(response_complete(b"HTTP/1.1 200 OK\r\n\r\n").is_err());
    }

    #[test]
    fn reload_commits_a_confirmed_snapshot_and_reports_status() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}",
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            ],
        );

        let status = control.reload().unwrap();
        server.join().unwrap();
        assert_eq!(status.child_pid, Some(41));
        assert!(status.tls_ready);
        assert_eq!(status.last_reload, "ok");
        assert!(status.config_sha256.is_some());
        assert_eq!(
            confirmed_snapshot(&config(), temp.path()),
            Some(temp.path().join("config-state/current.json"))
        );
        assert!(temp.path().join("managed.caddyfile").is_file());
    }

    #[test]
    fn validation_does_not_replace_snapshot_and_reload_failure_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}"],
        );
        control.validate().unwrap();
        server.join().unwrap();
        assert!(confirmed_snapshot(&config(), temp.path()).is_none());

        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n"],
        );
        assert!(control.reload().is_err());
        server.join().unwrap();
        let status = control.status();
        assert_eq!(status.last_reload, "failed");
        assert_eq!(status.last_error.as_deref(), Some("adapt_failed"));
        control.record_dns_result(true);
        assert_eq!(control.status().dns, "ok");
    }
}
