use super::super::SecretReference;
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use url::Url;

const MAX_HEADERS: usize = 16;
const MAX_HEADER_NAME_BYTES: usize = 64;
const MAX_HEADER_VALUE_BYTES: usize = 4 * 1024;
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// One operation-specific OpenAI-compatible backend.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiBackendConfig {
    /// Closed request and response protocol implemented by this backend.
    pub protocol: AiBackendProtocol,
    /// Canonical final request URL; no route is appended at runtime.
    pub endpoint: String,
    /// Explicit authentication policy.
    pub auth: AiAuthConfig,
    /// Bounded non-sensitive static metadata headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl AiBackendConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        let url = canonical_endpoint(&self.endpoint)?;
        self.auth.validate()?;
        let auth_header = self.auth.header_name();
        validate_headers(&self.headers, auth_header.as_deref())?;
        if self.auth == AiAuthConfig::None
            && (url.scheme() != "http" || !url_host_is_loopback(&url))
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI backend auth kind none is allowed only for loopback HTTP",
            ));
        }
        Ok(())
    }
}

/// Closed OpenAI-compatible operation protocols.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiBackendProtocol {
    /// OpenAI-compatible embeddings request and response shapes.
    #[serde(rename = "openai_embeddings_v1")]
    OpenAiEmbeddingsV1,
    /// OpenAI-compatible chat-completions and SSE shapes.
    #[serde(rename = "openai_chat_completions_v1")]
    OpenAiChatCompletionsV1,
}

impl AiBackendProtocol {
    /// Stable token frozen into model contracts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiEmbeddingsV1 => "openai_embeddings_v1",
            Self::OpenAiChatCompletionsV1 => "openai_chat_completions_v1",
        }
    }
}

/// Explicit backend authentication policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AiAuthConfig {
    /// Send one resolved secret as an HTTP Bearer credential.
    Bearer {
        /// Symbolic secret reference; its value never enters config serialization or contracts.
        secret: SecretReference,
    },
    /// Send one resolved secret as the complete value of a custom header.
    Header {
        /// Header name; `Authorization` and platform-owned headers are forbidden.
        name: String,
        /// Symbolic secret reference; its value never enters config serialization or contracts.
        secret: SecretReference,
    },
    /// Send no credential; accepted only for explicit loopback HTTP.
    None,
}

impl AiAuthConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        match self {
            Self::Bearer { secret } => secret.validate("ai.backends.*.auth.secret"),
            Self::Header { name, secret } => {
                let normalized = normalize_header_name(name)?;
                if reserved_header(&normalized) {
                    return Err(invalid_header());
                }
                secret.validate("ai.backends.*.auth.secret")
            }
            Self::None => Ok(()),
        }
    }

    /// Stable secret-free token included in the backend contract.
    #[must_use]
    pub const fn kind_token(&self) -> &'static str {
        match self {
            Self::Bearer { .. } => "bearer",
            Self::Header { .. } => "header",
            Self::None => "none",
        }
    }

    /// Normalized custom credential header name, when configured.
    #[must_use]
    pub fn header_name(&self) -> Option<String> {
        match self {
            Self::Header { name, .. } => Some(name.to_ascii_lowercase()),
            Self::Bearer { .. } | Self::None => None,
        }
    }
}

pub(super) fn canonical_endpoint(value: &str) -> Result<Url, PlatformError> {
    let url = Url::parse(value).map_err(|_| invalid_backend_url())?;
    let path = url.path();
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || path == "/"
        || path.ends_with('/')
        || path.contains("//")
        || value != url.as_str()
        || (url.scheme() == "http" && !url_host_is_loopback(&url))
    {
        return Err(invalid_backend_url());
    }
    Ok(url)
}

pub(super) fn headers_digest(headers: &BTreeMap<String, String>) -> Result<String, PlatformError> {
    #[derive(Serialize)]
    struct Entry<'a> {
        name: String,
        value: &'a str,
    }

    let mut entries = headers
        .iter()
        .map(|(name, value)| Entry {
            name: name.to_ascii_lowercase(),
            value,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let bytes = serde_json::to_vec(&entries).map_err(|_| invalid_header())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn url_host_is_loopback(url: &Url) -> bool {
    url.host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}

fn validate_headers(
    headers: &BTreeMap<String, String>,
    auth_header_name: Option<&str>,
) -> Result<(), PlatformError> {
    if headers.len() > MAX_HEADERS {
        return Err(invalid_header());
    }
    let mut normalized = BTreeSet::new();
    let mut bytes = 0_usize;
    for (name, value) in headers {
        let name = normalize_header_name(name)?;
        if reserved_header(&name)
            || auth_header_name == Some(name.as_str())
            || !normalized.insert(name.clone())
            || value.is_empty()
            || value.len() > MAX_HEADER_VALUE_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(invalid_header());
        }
        bytes = bytes
            .checked_add(name.len())
            .and_then(|total| total.checked_add(value.len()))
            .ok_or_else(invalid_header)?;
    }
    if bytes > MAX_HEADER_BYTES {
        return Err(invalid_header());
    }
    Ok(())
}

fn normalize_header_name(value: &str) -> Result<String, PlatformError> {
    if value.is_empty()
        || value.len() > MAX_HEADER_NAME_BYTES
        || !value.bytes().all(is_header_name_byte)
    {
        return Err(invalid_header());
    }
    Ok(value.to_ascii_lowercase())
}

const fn is_header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn reserved_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "host"
            | "content-length"
            | "content-type"
            | "accept"
            | "user-agent"
            | "cookie"
            | "connection"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
            | "proxy-authorization"
            | "proxy-authenticate"
    ) || name.starts_with("proxy-")
}

fn invalid_backend_url() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI backend endpoint must be a canonical HTTPS or loopback HTTP final URL",
    )
}

fn invalid_header() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI backend header contract is invalid",
    )
}
