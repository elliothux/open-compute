//! Instance-owned Browser Run configuration, independent of tenant bindings.

use crate::{ErrorCode, PlatformError, SecretReference};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

/// Explicit admission, transport, and execution bounds for one instance.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrowserConfig {
    /// Operator-owned HTTP(S) control origin for remote CDP and Live View clients.
    /// When absent, URLs use the bound loopback control listener.
    #[serde(default)]
    pub public_origin: Option<String>,
    /// Maximum simultaneously retained browser sessions.
    pub max_sessions: u32,
    /// Maximum callers waiting for session admission.
    pub max_pending_acquires: u32,
    /// Admission deadline, including process startup and context allocation.
    pub acquire_timeout_ms: u64,
    /// End-to-end command and Quick Action deadline.
    pub command_timeout_ms: u64,
    /// Maximum simultaneous client CDP connections per instance; control channels are session-bounded.
    pub max_connections: u32,
    /// Maximum simultaneous frontend resource requests, including bounded capacity waiters.
    pub max_frontend_requests: u32,
    /// Maximum simultaneous Quick Actions per instance.
    pub max_actions: u32,
    /// Maximum HTTP request body bytes.
    pub max_body_bytes: u64,
    /// Maximum action output bytes.
    pub max_result_bytes: u64,
    /// Maximum retained download bytes per managed session, checked every 100 milliseconds.
    pub max_download_bytes: u64,
    /// Maximum retained download files and download identities per managed session.
    pub max_download_files: u32,
    /// Maximum assembled CDP message bytes.
    pub max_message_bytes: u64,
    /// Maximum queued CDP output messages per connection.
    pub max_queued_messages: u32,
    /// Maximum retained closed or lost session metadata records per instance.
    pub max_history_entries: u32,
    /// Retention after a session becomes closed or lost, in milliseconds.
    pub history_retention_ms: u64,
    /// Operator-selected backend; tenant requests cannot override it.
    pub backend: BrowserBackendConfig,
}

/// Exactly one operator-owned browser backend.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserBackendConfig {
    /// One on-demand supervised browser process group per instance.
    Managed {
        /// Absolute executable path within its complete installation directory.
        executable: PathBuf,
        /// Time to retain a browser generation after all work is released.
        browser_idle_timeout_ms: u64,
        /// Grace before forced process-group termination.
        shutdown_grace_ms: u64,
    },
    /// Native attachment to an operator-managed browser-level CDP endpoint.
    Cdp {
        /// HTTP(S) discovery URL or browser-level WS(S) URL, without credentials.
        url: String,
        /// Optional bearer credential resolved only by the service authority.
        #[serde(default)]
        authorization: Option<SecretReference>,
    },
}

impl BrowserConfig {
    /// Validate all bounds without launching processes or resolving credentials.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if let Some(value) = &self.public_origin {
            let origin = Url::parse(value).map_err(|_| invalid_backend())?;
            if value.len() > 4_096
                || value
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
                || !matches!(origin.scheme(), "http" | "https")
                || origin.host_str().is_none()
                || !origin.username().is_empty()
                || origin.password().is_some()
                || origin.path() != "/"
                || origin.query().is_some()
                || origin.fragment().is_some()
            {
                return Err(invalid_backend());
            }
        }
        for (value, maximum) in [
            (u64::from(self.max_sessions), 1_024),
            (u64::from(self.max_pending_acquires), 4_096),
            (self.acquire_timeout_ms, 120_000),
            (self.command_timeout_ms, 120_000),
            (u64::from(self.max_connections), 4_096),
            (u64::from(self.max_frontend_requests), 1_024),
            (u64::from(self.max_actions), 1_024),
            (self.max_body_bytes, 16 * 1024 * 1024),
            (self.max_result_bytes, 128 * 1024 * 1024),
            (self.max_download_bytes, 128 * 1024 * 1024),
            (u64::from(self.max_download_files), 4_096),
            (self.max_message_bytes, 16 * 1024 * 1024),
            (u64::from(self.max_queued_messages), 1_024),
            (u64::from(self.max_history_entries), 100_000),
            (self.history_retention_ms, 31_536_000_000),
        ] {
            if value == 0 || value > maximum {
                return Err(PlatformError::new(
                    ErrorCode::LimitInvalid,
                    "browser limits must be nonzero and within supported bounds",
                ));
            }
        }
        match &self.backend {
            BrowserBackendConfig::Managed {
                executable,
                browser_idle_timeout_ms,
                shutdown_grace_ms,
            } => {
                super::require_absolute(executable, "browser.backend.executable")?;
                if executable.file_name().is_none()
                    || !(1..=3_600_000).contains(browser_idle_timeout_ms)
                    || !(1..=30_000).contains(shutdown_grace_ms)
                {
                    return Err(invalid_backend());
                }
            }
            BrowserBackendConfig::Cdp { url, authorization } => {
                validate_cdp_url(url)?;
                if let Some(reference) = authorization {
                    reference.validate("browser.backend.authorization")?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn resolve_paths(&mut self, base: &std::path::Path) -> Result<(), PlatformError> {
        if let BrowserBackendConfig::Cdp {
            authorization: Some(reference),
            ..
        } = &mut self.backend
        {
            super::resolve_secret_path(base, reference)?;
        }
        Ok(())
    }
}

/// Reject page endpoints, embedded credentials, fragments, and unsupported schemes.
pub fn validate_cdp_url(value: &str) -> Result<Url, PlatformError> {
    if value.len() > 4_096 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(invalid_backend());
    }
    let url = Url::parse(value).map_err(|_| invalid_backend())?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || match url.scheme() {
            "http" | "https" => !matches!(url.path(), "" | "/" | "/json/version"),
            "ws" | "wss" => !url
                .path()
                .strip_prefix("/devtools/browser/")
                .is_some_and(|id| {
                    !id.is_empty()
                        && id.len() <= 256
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                }),
            _ => true,
        }
    {
        return Err(invalid_backend());
    }
    Ok(url)
}

fn invalid_backend() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "invalid browser backend configuration",
    )
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
