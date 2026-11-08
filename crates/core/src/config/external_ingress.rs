use crate::{ErrorCode, PlatformError};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

const MAX_TRUSTED_PROXIES: usize = 64;
const MAX_EXTERNAL_HOSTNAMES: usize = 256;

/// The one optional tenant-only listener that an operator reverse proxy forwards to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonExternalIngressConfig {
    /// Plain HTTP bind address; it has no default port.
    pub bind: SocketAddr,
    /// TCP peers allowed to connect, as explicit IP networks.
    pub trusted_proxies: Vec<IpNet>,
}

impl DaemonExternalIngressConfig {
    /// Validate the bind, the peer allowlist, and conflicts with the other daemon listeners.
    pub fn validate(&self, others: &[SocketAddr]) -> Result<(), PlatformError> {
        if self.bind.port() == 0 {
            return Err(invalid("external_ingress.bind requires a fixed port"));
        }
        if others
            .iter()
            .any(|other| listeners_overlap(self.bind, *other))
        {
            return Err(invalid(
                "external_ingress.bind conflicts with another daemon listener",
            ));
        }
        if self.trusted_proxies.is_empty() || self.trusted_proxies.len() > MAX_TRUSTED_PROXIES {
            return Err(invalid(
                "external_ingress.trusted_proxies needs 1 to 64 networks",
            ));
        }
        for network in &self.trusted_proxies {
            if network.prefix_len() == 0 {
                return Err(invalid("external_ingress.trusted_proxies must be scoped"));
            }
            if let IpNet::V6(v6) = network
                && v6.addr().to_ipv4_mapped().is_some()
            {
                return Err(invalid(
                    "external_ingress.trusted_proxies must write IPv4 peers as IPv4",
                ));
            }
        }
        Ok(())
    }

    /// Whether one accepted TCP peer is in the allowlist, comparing IPv4-mapped peers as IPv4.
    #[must_use]
    pub fn trusts(&self, peer: IpAddr) -> bool {
        let peer = peer.to_canonical();
        self.trusted_proxies
            .iter()
            .any(|network| network.contains(&peer))
    }
}

fn listeners_overlap(left: SocketAddr, right: SocketAddr) -> bool {
    left.port() == right.port()
        && (left.ip() == right.ip() || left.ip().is_unspecified() || right.ip().is_unspecified())
}

/// Exact hostnames that the operator delegates to one instance for external proxy origins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalIngressConfig {
    /// Canonical exact hostnames; no wildcard and no implied subdomains.
    pub hostnames: Vec<String>,
}

impl ExternalIngressConfig {
    /// Validate each hostname once and reject duplicates.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.hostnames.len() > MAX_EXTERNAL_HOSTNAMES {
            return Err(invalid(
                "external_ingress.hostnames allows at most 256 hostnames",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for hostname in &self.hostnames {
            validate_external_hostname(hostname)?;
            if !seen.insert(hostname) {
                return Err(invalid("external_ingress.hostnames contains a duplicate"));
            }
        }
        Ok(())
    }
}

/// Validate one canonical exact hostname for an external proxy origin.
///
/// The hostname is lowercase ASCII (A-labels for IDN), has no trailing dot, has at least
/// two labels, and is not an IP literal or a `localhost` name.
pub fn validate_external_hostname(value: &str) -> Result<(), PlatformError> {
    let error = || invalid("external hostname must be a canonical DNS name");
    let labels = value.split('.').collect::<Vec<_>>();
    if value.len() < 4
        || value.len() > 253
        || labels.len() < 2
        || !labels.iter().all(|label| {
            let bytes = label.as_bytes();
            !bytes.is_empty()
                && bytes.len() <= 63
                && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
                && bytes
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        })
    {
        return Err(error());
    }
    if value == "localhost" || value.ends_with(".localhost") {
        return Err(invalid("external hostname cannot be a localhost name"));
    }
    match url::Host::parse(value) {
        Ok(url::Host::Domain(domain)) if domain == value => Ok(()),
        _ => Err(error()),
    }
}

/// Scheme an operator proxy promises to serve for one external origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalScheme {
    /// Plain HTTP.
    Http,
    /// HTTPS terminated by the operator proxy.
    Https,
}

impl ExternalScheme {
    /// Persisted and URL token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    /// Default port of the scheme.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }

    /// Parse the persisted token.
    pub fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            _ => Err(invalid("external origin scheme must be http or https")),
        }
    }
}

/// One validated external origin: scheme, canonical hostname, and explicit public port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalOrigin {
    /// Public scheme.
    pub scheme: ExternalScheme,
    /// Canonical exact hostname.
    pub hostname: String,
    /// Public port, with the scheme default stored explicitly.
    pub port: u16,
}

impl ExternalOrigin {
    /// Parse `scheme://host[:port][/]` with no userinfo, query, fragment, or path.
    pub fn parse(value: &str) -> Result<Self, PlatformError> {
        let error = || invalid("external origin must be http(s)://hostname[:port]");
        let (scheme, rest) = if let Some(rest) = value.strip_prefix("https://") {
            (ExternalScheme::Https, rest)
        } else if let Some(rest) = value.strip_prefix("http://") {
            (ExternalScheme::Http, rest)
        } else {
            return Err(error());
        };
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.contains(['/', '?', '#', '@', '[', ']', '\\', '%', '*']) {
            return Err(error());
        }
        let (hostname, port) = match authority.split_once(':') {
            Some((hostname, port)) => (hostname, parse_port(port).ok_or_else(error)?),
            None => (authority, scheme.default_port()),
        };
        validate_external_hostname(hostname)?;
        Ok(Self {
            scheme,
            hostname: hostname.to_owned(),
            port,
        })
    }

    /// Canonical origin text: no default port and no trailing slash.
    #[must_use]
    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme.as_str(), self.authority())
    }

    /// Canonical authority: hostname, plus the port only when it is not the default.
    #[must_use]
    pub fn authority(&self) -> String {
        if self.port == self.scheme.default_port() {
            self.hostname.clone()
        } else {
            format!("{}:{}", self.hostname, self.port)
        }
    }
}

fn parse_port(value: &str) -> Option<u16> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u16>().ok().filter(|port| *port != 0)
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_origin_accepts_only_canonical_root_origins() {
        let origin = ExternalOrigin::parse("https://app.example.com").unwrap();
        assert_eq!(origin.port, 443);
        assert_eq!(origin.origin(), "https://app.example.com");
        for (input, canonical) in [
            ("https://app.example.com/", "https://app.example.com"),
            ("https://app.example.com:443", "https://app.example.com"),
            ("https://example.com:8443/", "https://example.com:8443"),
            ("http://app.example.com:8080", "http://app.example.com:8080"),
            (
                "http://xn--bcher-kva.example",
                "http://xn--bcher-kva.example",
            ),
        ] {
            assert_eq!(ExternalOrigin::parse(input).unwrap().origin(), canonical);
        }
        for input in [
            "ftp://app.example.com",
            "HTTPS://app.example.com",
            "https://App.example.com",
            "https://app.example.com.",
            "https://app.example.com/app",
            "https://app.example.com?x=1",
            "https://app.example.com#x",
            "https://user:pass@app.example.com",
            "https://*.example.com",
            "https://127.0.0.1",
            "https://[::1]",
            "https://app.example.com:0",
            "https://app.example.com:65536",
            "https://app.example.com:+80",
            "https://",
            "https://localhost",
            "https://app.localhost",
            "https://example",
        ] {
            assert!(ExternalOrigin::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn daemon_external_ingress_rejects_unscoped_or_conflicting_listeners() {
        let config = DaemonExternalIngressConfig {
            bind: "127.0.0.1:8788".parse().unwrap(),
            trusted_proxies: vec!["127.0.0.1/32".parse().unwrap()],
        };
        config
            .validate(&["127.0.0.1:8787".parse().unwrap()])
            .unwrap();
        assert!(config.validate(&["0.0.0.0:8788".parse().unwrap()]).is_err());
        assert!(config.trusts("127.0.0.1".parse().unwrap()));
        assert!(config.trusts("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!config.trusts("127.0.0.2".parse().unwrap()));
        for proxies in [
            vec![],
            vec!["0.0.0.0/0".parse().unwrap()],
            vec!["::/0".parse().unwrap()],
            vec!["::ffff:127.0.0.1/128".parse().unwrap()],
        ] {
            let config = DaemonExternalIngressConfig {
                trusted_proxies: proxies,
                ..config.clone()
            };
            assert!(config.validate(&[]).is_err());
        }
        let zero = DaemonExternalIngressConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..config
        };
        assert!(zero.validate(&[]).is_err());
    }

    #[test]
    fn instance_hostnames_are_exact_and_unique() {
        let config = ExternalIngressConfig {
            hostnames: vec!["example.com".to_owned(), "app.example.com".to_owned()],
        };
        config.validate().unwrap();
        let duplicate = ExternalIngressConfig {
            hostnames: vec!["example.com".to_owned(), "example.com".to_owned()],
        };
        assert!(duplicate.validate().is_err());
    }
}
