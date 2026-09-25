//! Operator-authorized loopback source providers for AI Search extensions.

use super::super::SecretReference;
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// One closed loopback HTTP source-provider authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiSourceProviderConfig {
    /// Canonical loopback base URL without a trailing slash.
    pub endpoint: String,
    /// Fixed application-owned source namespace.
    pub source: String,
    /// Bearer credential resolved only by the operator process.
    pub credential: SecretReference,
    /// Maximum exact source revision bytes.
    pub max_source_bytes: u64,
}

impl AiSourceProviderConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| invalid())?;
        if url.scheme() != "http"
            || url.port().is_none()
            || url
                .host_str()
                .and_then(|host| host.parse::<IpAddr>().ok())
                .is_none_or(|ip| !ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || (url.path() != "/" && url.path().ends_with('/'))
            || self.endpoint != url.as_str()
            || self.source.is_empty()
            || self.source.len() > 128
            || !self.source.is_ascii()
            || self.source.trim() != self.source
            || self.source.chars().any(char::is_control)
            || self.max_source_bytes == 0
            || self.max_source_bytes > 64 * 1024 * 1024
        {
            return Err(invalid());
        }
        self.credential.validate("ai.source_providers.*.credential")
    }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI Search source provider configuration is invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_provider_is_loopback_scoped_and_closed() {
        let valid = AiSourceProviderConfig {
            endpoint: "http://127.0.0.1:8090/provider".to_owned(),
            source: "files".to_owned(),
            credential: SecretReference {
                env: Some("SOURCE_TOKEN".to_owned()),
                file: None,
            },
            max_source_bytes: 1024,
        };
        assert!(valid.validate().is_ok());
        let mut root = valid.clone();
        root.endpoint = "http://127.0.0.1:8090/".to_owned();
        assert!(root.validate().is_ok());
        for endpoint in [
            "https://127.0.0.1:8090/provider",
            "http://localhost:8090/provider",
            "http://10.0.0.1:8090/provider",
            "http://127.0.0.1:8090/provider/",
        ] {
            let mut invalid = valid.clone();
            invalid.endpoint = endpoint.to_owned();
            assert!(invalid.validate().is_err());
        }
        let mut obsolete = serde_json::to_value(&valid).unwrap();
        obsolete["account_ids"] = serde_json::json!(["01994dc17a1070008000000000000001"]);
        assert!(serde_json::from_value::<AiSourceProviderConfig>(obsolete).is_err());
    }
}
