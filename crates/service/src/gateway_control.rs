//! Serialized managed Caddy configuration and secret-free runtime status.

use open_compute_core::{DaemonGatewayConfig, ErrorCode, PlatformError};
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
    domains: Vec<String>,
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
    config_sha256: String,
}

/// Synchronous control owner called only through the local operator socket.
pub(crate) struct GatewayControl {
    shared: DaemonGatewayConfig,
    gateway_dir: PathBuf,
    admin_path: PathBuf,
    upstream_path: PathBuf,
    provider_path: PathBuf,
    child_pid: std::sync::Arc<AtomicI32>,
    qualified_pid: std::sync::Arc<AtomicI32>,
    reload: Mutex<ReloadState>,
}

impl GatewayControl {
    /// Construct the one gateway control owner.
    #[allow(
        clippy::too_many_arguments,
        reason = "shared Gateway resources are passed explicitly"
    )]
    pub(crate) fn new(
        shared: DaemonGatewayConfig,
        domains: Vec<String>,
        gateway_dir: PathBuf,
        admin_path: PathBuf,
        upstream_path: PathBuf,
        provider_path: PathBuf,
        child_pid: std::sync::Arc<AtomicI32>,
        qualified_pid: std::sync::Arc<AtomicI32>,
    ) -> Self {
        let digest = snapshot_digest(&gateway_dir.join("config-state/current.json"));
        Self {
            shared,
            gateway_dir,
            admin_path,
            upstream_path,
            provider_path,
            child_pid,
            qualified_pid,
            reload: Mutex::new(ReloadState {
                domains,
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

    pub(crate) fn domains(&self) -> Result<Vec<String>, PlatformError> {
        Ok(self
            .reload
            .lock()
            .map_err(|_| control_error("gateway reload state is unavailable"))?
            .domains
            .clone())
    }

    pub(crate) fn validate_domains(&self, domains: &[String]) -> Result<(), PlatformError> {
        crate::gateway_caddyfile::render(
            &self.shared,
            domains,
            &self.gateway_dir,
            &self.admin_path,
            &self.upstream_path,
            &self.provider_path,
        )?;
        Ok(())
    }

    /// Adapt, validate, load, and commit one complete configuration.
    pub(crate) fn reload(&self) -> Result<GatewayStatus, PlatformError> {
        let domains = self.domains()?;
        self.reload_domains(domains)
    }

    /// Replace the full domain projection only after Caddy accepts it.
    pub(crate) fn reload_domains(
        &self,
        domains: Vec<String>,
    ) -> Result<GatewayStatus, PlatformError> {
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
        let result = self.reload_candidate(&candidate, &domains, &mut state);
        match result {
            Ok(digest) => {
                let _ = fs::remove_dir_all(&candidate);
                state.domains = domains;
                self.qualified_pid.store(0, Ordering::Release);
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
        let result = self.adapt_candidate(&candidate, &guard.domains);
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
        domains: &[String],
        state: &mut ReloadState,
    ) -> Result<String, PlatformError> {
        crate::gateway_certificates::check_certified_domains(&self.gateway_dir, domains)?;
        state.last_error = Some("adapt_failed");
        let adapted = self.adapt_candidate(candidate, domains)?;
        state.last_error = Some("load_failed");
        open_compute_storage::atomic_write(&candidate.join("candidate.json"), &adapted)?;
        admin_request(
            &self.admin_path,
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
            schema_version: 2,
            intent_sha256: intent_digest(&self.shared, domains)?,
            payload_sha256: open_compute_runtime::embedded_payload_sha256().to_owned(),
            config_sha256: hex::encode(Sha256::digest(&adapted)),
        };
        let meta = serde_json::to_vec(&meta)
            .map_err(|_| control_error("failed to encode Caddy snapshot metadata"))?;
        open_compute_storage::atomic_write(&state_dir.join("current.meta.json"), &meta)?;
        crate::gateway_caddyfile::write_managed(
            &self.shared,
            domains,
            &self.gateway_dir,
            &self.admin_path,
            &self.upstream_path,
            &self.provider_path,
        )?;
        Ok(hex::encode(Sha256::digest(&adapted)))
    }

    fn adapt_candidate(
        &self,
        candidate: &Path,
        domains: &[String],
    ) -> Result<Vec<u8>, PlatformError> {
        let managed_path = candidate.join("managed.caddyfile");
        let (managed, entrypoint) = crate::gateway_caddyfile::render_candidate(
            &self.shared,
            domains,
            &self.gateway_dir,
            &managed_path,
            &self.admin_path,
            &self.upstream_path,
            &self.provider_path,
        )?;
        open_compute_storage::atomic_write(&managed_path, managed.as_bytes())?;
        open_compute_storage::atomic_write(&candidate.join("Caddyfile"), entrypoint.as_bytes())?;
        let source = fs::read(candidate.join("Caddyfile"))
            .map_err(|_| control_error("failed to read Caddy reload candidate"))?;
        let adapted_response = admin_request(
            &self.admin_path,
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
    shared: &DaemonGatewayConfig,
    domains: &[String],
    gateway_dir: &Path,
) -> Option<PathBuf> {
    let bytes = fs::read(gateway_dir.join("config-state/current.meta.json")).ok()?;
    let meta: SnapshotMeta = serde_json::from_slice(&bytes).ok()?;
    if meta.schema_version != 2
        || meta.payload_sha256 != open_compute_runtime::embedded_payload_sha256()
        || meta.intent_sha256 != intent_digest(shared, domains).ok()?
    {
        return None;
    }
    let snapshot = gateway_dir.join("config-state/current.json");
    let bytes = fs::read(&snapshot).ok()?;
    if hex::encode(Sha256::digest(&bytes)) != meta.config_sha256 {
        return None;
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    Some(snapshot)
}

fn admin_request(
    socket: &Path,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> Result<Vec<u8>, PlatformError> {
    if !crate::instance_control::unix_socket_path_is_valid(socket) {
        return Err(control_error("managed Caddy admin socket path is invalid"));
    }
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
    let body = &response[split + 4..];
    if headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    }) {
        return decode_chunked(body);
    }
    if let Some(length) = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    }) {
        return body
            .get(..length)
            .map(ToOwned::to_owned)
            .ok_or_else(|| control_error("managed Caddy response was truncated"));
    }
    Ok(body.to_vec())
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, PlatformError> {
    let mut decoded = Vec::new();
    let mut rest = body;
    loop {
        let split = rest
            .windows(2)
            .position(|bytes| bytes == b"\r\n")
            .ok_or_else(|| control_error("managed Caddy chunked response is invalid"))?;
        let size = std::str::from_utf8(&rest[..split])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|value| usize::from_str_radix(value.trim(), 16).ok())
            .ok_or_else(|| control_error("managed Caddy chunked response is invalid"))?;
        rest = &rest[split + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        let chunk = rest
            .get(..size)
            .ok_or_else(|| control_error("managed Caddy chunked response is truncated"))?;
        decoded.extend_from_slice(chunk);
        rest = rest
            .get(size..)
            .and_then(|tail| tail.strip_prefix(b"\r\n"))
            .ok_or_else(|| control_error("managed Caddy chunked response is invalid"))?;
    }
}

fn snapshot_digest(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(hex::encode(Sha256::digest(bytes)))
}

fn intent_digest(
    shared: &DaemonGatewayConfig,
    domains: &[String],
) -> Result<String, PlatformError> {
    let bytes = serde_json::to_vec(&(shared, domains))
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
            shared: DaemonGatewayConfig {
                ingress_ipv4: vec![Ipv4Addr::new(203, 0, 113, 10)],
                ingress_ipv6: Vec::new(),
                https_listen: "127.0.0.1:8443".parse().unwrap(),
                challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
                proxy_protocol_from: Vec::new(),
                caddy: Vec::new(),
            },
        }
    }

    fn control(root: &Path) -> GatewayControl {
        for directory in ["run", "storage", "config-state"] {
            open_compute_storage::ensure_dir_secure(&root.join(directory)).unwrap();
        }
        crate::gateway_certificates::initialize_registry(root).unwrap();
        GatewayControl::new(
            config().shared,
            vec!["compute.example.com".to_owned()],
            root.to_owned(),
            root.join("run/admin.sock"),
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
    fn chunked_admin_response_is_bounded_and_rejects_truncation() {
        assert_eq!(decode_chunked(b"2\r\n{}\r\n0\r\n\r\n").unwrap(), b"{}");
        assert!(decode_chunked(b"2\r\n{\r\n0\r\n\r\n").is_err());
        assert!(decode_chunked(b"bad\r\n").is_err());
    }

    #[test]
    fn certified_site_loss_rejects_reload_before_contacting_caddy() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let site = temp
            .path()
            .join("storage/certificates/issuer/wildcard_.compute.example.com");
        for dir in [
            temp.path().join("storage/certificates"),
            temp.path().join("storage/certificates/issuer"),
            site.clone(),
        ] {
            open_compute_storage::ensure_dir_secure(&dir).unwrap();
        }
        for suffix in ["crt", "key", "json"] {
            open_compute_storage::atomic_write(
                &site.join(format!("wildcard_.compute.example.com.{suffix}")),
                b"asset",
            )
            .unwrap();
        }
        crate::gateway_certificates::record_certified_domain(temp.path(), "compute.example.com")
            .unwrap();
        fs::remove_dir_all(site).unwrap();
        assert_eq!(
            control.reload().unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        assert!(!temp.path().join("config-state/current.json").exists());
    }

    #[test]
    fn reload_commits_a_confirmed_snapshot_and_reports_status() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nD\r\n{\"result\":{}}\r\n0\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            ],
        );

        let status = control.reload().unwrap();
        server.join().unwrap();
        assert_eq!(status.child_pid, Some(41));
        assert!(!status.tls_ready);
        assert_eq!(status.last_reload, "ok");
        assert!(status.config_sha256.is_some());
        assert_eq!(
            confirmed_snapshot(&config().shared, &[config().base_domain], temp.path()),
            Some(temp.path().join("config-state/current.json"))
        );
        assert!(temp.path().join("managed.caddyfile").is_file());
        fs::write(
            temp.path().join("config-state/current.json"),
            b"{\"apps\":{}}",
        )
        .unwrap();
        assert!(
            confirmed_snapshot(&config().shared, &[config().base_domain], temp.path()).is_none()
        );
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
        assert!(
            confirmed_snapshot(&config().shared, &[config().base_domain], temp.path()).is_none()
        );

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

    #[test]
    fn domain_replacement_commits_new_intent_and_accepts_no_domains() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}",
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}",
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            ],
        );
        let replacement = vec!["other.example.net".to_owned()];
        control.reload_domains(replacement.clone()).unwrap();
        assert_eq!(control.domains().unwrap(), replacement);
        assert!(confirmed_snapshot(&config().shared, &replacement, temp.path()).is_some());
        assert!(
            confirmed_snapshot(&config().shared, &[config().base_domain], temp.path()).is_none()
        );
        control.reload_domains(Vec::new()).unwrap();
        server.join().unwrap();
        assert!(control.domains().unwrap().is_empty());
        assert!(confirmed_snapshot(&config().shared, &[], temp.path()).is_some());
        assert!(
            !fs::read_to_string(temp.path().join("managed.caddyfile"))
                .unwrap()
                .contains("reverse_proxy")
        );
    }

    #[test]
    fn invalid_domain_replacement_preserves_confirmed_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let control = control(temp.path());
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}",
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            ],
        );
        control.reload().unwrap();
        server.join().unwrap();
        let current = fs::read(temp.path().join("config-state/current.json")).unwrap();
        let managed = fs::read(temp.path().join("managed.caddyfile")).unwrap();
        assert!(
            control
                .reload_domains(vec![
                    "compute.example.com".to_owned(),
                    "nested.compute.example.com".to_owned(),
                ])
                .is_err()
        );
        assert_eq!(control.domains().unwrap(), vec!["compute.example.com"]);
        assert_eq!(
            fs::read(temp.path().join("config-state/current.json")).unwrap(),
            current
        );
        assert_eq!(
            fs::read(temp.path().join("managed.caddyfile")).unwrap(),
            managed
        );
        let server = serve_admin(
            &temp.path().join("run/admin.sock"),
            vec![
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"result\":{}}",
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            ],
        );
        assert!(
            control
                .reload_domains(vec!["other.example.net".to_owned()])
                .is_err()
        );
        server.join().unwrap();
        assert_eq!(control.domains().unwrap(), vec!["compute.example.com"]);
        assert_eq!(
            fs::read(temp.path().join("config-state/current.json")).unwrap(),
            current
        );
        assert_eq!(
            fs::read(temp.path().join("managed.caddyfile")).unwrap(),
            managed
        );
    }
}
