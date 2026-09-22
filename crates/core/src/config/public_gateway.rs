use crate::{ErrorCode, PlatformError};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use url::Host;

/// One operator-owned Caddyfile imported into the managed gateway configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaddyFileConfig {
    /// Source file, resolved relative to the platform config file.
    pub caddy_file: PathBuf,
}

/// Static intent for one optional public gateway and one base domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicGatewayConfig {
    /// Exclusive canonical base domain for platform public origins.
    pub base_domain: String,
    /// Public IPv4 addresses used in the DNS setup plan.
    #[serde(default)]
    pub ingress_ipv4: Vec<Ipv4Addr>,
    /// Public IPv6 addresses used in the DNS setup plan.
    #[serde(default)]
    pub ingress_ipv6: Vec<Ipv6Addr>,
    /// Caddy HTTPS bind address; external transport may forward TCP 443 here.
    pub https_listen: SocketAddr,
    /// Authoritative challenge DNS bind address for both UDP and TCP.
    pub challenge_dns_listen: SocketAddr,
    /// Exact proxy peers or networks allowed to supply PROXY protocol metadata.
    #[serde(default)]
    pub proxy_protocol_from: Vec<IpNet>,
    /// Additional operator-managed Caddyfiles.
    #[serde(default)]
    pub caddy: Vec<CaddyFileConfig>,
}

/// One operator-created DNS record in the static public gateway setup plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GatewayDnsRecord {
    /// Absolute record owner name without a trailing dot.
    pub name: String,
    /// Standard DNS record type.
    pub kind: GatewayDnsRecordKind,
    /// Address or absolute DNS target without a trailing dot.
    pub value: String,
}

/// DNS record type used by the public gateway setup plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GatewayDnsRecordKind {
    /// IPv4 address.
    A,
    /// IPv6 address.
    Aaaa,
    /// Canonical alias.
    Cname,
    /// Authoritative delegation.
    Ns,
}

impl PublicGatewayConfig {
    /// Deterministic records for Worker and optionally R2 public namespaces.
    #[must_use]
    pub fn dns_plan(&self, r2_enabled: bool) -> Vec<GatewayDnsRecord> {
        let base = &self.base_domain;
        let ingress = format!("ingress.{base}");
        let ns1 = format!("ns1.{base}");
        let mut records = Vec::new();
        for address in &self.ingress_ipv4 {
            records.push(GatewayDnsRecord {
                name: ingress.clone(),
                kind: GatewayDnsRecordKind::A,
                value: address.to_string(),
            });
            records.push(GatewayDnsRecord {
                name: ns1.clone(),
                kind: GatewayDnsRecordKind::A,
                value: address.to_string(),
            });
        }
        for address in &self.ingress_ipv6 {
            records.push(GatewayDnsRecord {
                name: ingress.clone(),
                kind: GatewayDnsRecordKind::Aaaa,
                value: address.to_string(),
            });
            records.push(GatewayDnsRecord {
                name: ns1.clone(),
                kind: GatewayDnsRecordKind::Aaaa,
                value: address.to_string(),
            });
        }
        for zone in [base.clone(), format!("r2.{base}")]
            .into_iter()
            .take(if r2_enabled { 2 } else { 1 })
        {
            records.push(GatewayDnsRecord {
                name: format!("*.{zone}"),
                kind: GatewayDnsRecordKind::Cname,
                value: ingress.clone(),
            });
            records.push(GatewayDnsRecord {
                name: format!("_acme-challenge.{zone}"),
                kind: GatewayDnsRecordKind::Ns,
                value: ns1.clone(),
            });
        }
        records
    }

    pub(super) fn normalize(&mut self) -> Result<(), PlatformError> {
        let input = self
            .base_domain
            .strip_suffix('.')
            .unwrap_or(&self.base_domain);
        self.base_domain = match Host::parse(input).map_err(|_| invalid("invalid base domain"))? {
            Host::Domain(domain) => domain.to_ascii_lowercase(),
            Host::Ipv4(_) | Host::Ipv6(_) => return Err(invalid("base domain must be a DNS name")),
        };
        Ok(())
    }

    pub(super) fn resolve_paths(&mut self, base: &Path) -> Result<(), PlatformError> {
        for file in &mut self.caddy {
            file.caddy_file = super::resolve_host_path(base, &file.caddy_file)?;
        }
        Ok(())
    }

    /// Validate the canonical domain, addresses, listeners, proxy peers, and file list.
    pub fn validate(&self) -> Result<(), PlatformError> {
        Self::validate_base_domain(&self.base_domain)?;
        if self.caddy.len() > 16 {
            return Err(invalid(
                "at most 16 public gateway Caddyfiles may be configured",
            ));
        }
        if self.ingress_ipv4.is_empty() && self.ingress_ipv6.is_empty() {
            return Err(invalid("public gateway requires an ingress IP address"));
        }
        if self.https_listen.port() == 0 || self.challenge_dns_listen.port() == 0 {
            return Err(invalid("public gateway listeners require fixed ports"));
        }
        if self.https_listen.port() == self.challenge_dns_listen.port()
            && (self.https_listen.ip() == self.challenge_dns_listen.ip()
                || self.https_listen.ip().is_unspecified()
                || self.challenge_dns_listen.ip().is_unspecified())
        {
            return Err(invalid(
                "gateway HTTPS and challenge DNS cannot share a TCP listener",
            ));
        }
        if self
            .proxy_protocol_from
            .iter()
            .any(|network| network.prefix_len() == 0)
        {
            return Err(invalid("PROXY protocol peers must be explicitly scoped"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for file in &self.caddy {
            super::require_absolute(&file.caddy_file, "public_gateway.caddy.caddy_file")?;
            if !seen.insert(&file.caddy_file) {
                return Err(invalid("duplicate public gateway Caddyfile"));
            }
        }
        Ok(())
    }

    /// Validate a canonical persisted base domain without other operator intent.
    pub fn validate_base_domain(base_domain: &str) -> Result<(), PlatformError> {
        if format!("_acme-challenge.r2.{base_domain}").len() > 253
            || base_domain.ends_with('.')
            || psl::domain_str(base_domain).is_none()
            || !matches!(Host::parse(base_domain), Ok(Host::Domain(domain)) if domain == base_domain)
            || !base_domain.split('.').all(|label| {
                let bytes = label.as_bytes();
                bytes.len() <= 63
                    && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                    && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
                    && bytes
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            })
        {
            return Err(invalid("base domain must be a registrable DNS name"));
        }
        Ok(())
    }
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_gateway_normalizes_and_rejects_unsafe_authority() {
        let mut config = PublicGatewayConfig {
            base_domain: "COMPUTE.Example.COM.".to_owned(),
            ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
            ingress_ipv6: vec!["2001:4860:4860::8888".parse().unwrap()],
            https_listen: "0.0.0.0:8443".parse().unwrap(),
            challenge_dns_listen: "0.0.0.0:8053".parse().unwrap(),
            proxy_protocol_from: Vec::new(),
            caddy: vec![CaddyFileConfig {
                caddy_file: PathBuf::from("/etc/open-compute/git.caddyfile"),
            }],
        };
        config.normalize().unwrap();
        config.validate().unwrap();
        assert_eq!(config.base_domain, "compute.example.com");
        let plan = config.dns_plan(true);
        assert!(plan.iter().any(|record| {
            record.name == "*.r2.compute.example.com"
                && record.kind == GatewayDnsRecordKind::Cname
                && record.value == "ingress.compute.example.com"
        }));
        assert!(plan.iter().any(|record| {
            record.name == "_acme-challenge.compute.example.com"
                && record.kind == GatewayDnsRecordKind::Ns
                && record.value == "ns1.compute.example.com"
        }));
        assert_eq!(config.dns_plan(false).len(), 6);
        config.base_domain = "Bücher.Example".to_owned();
        config.normalize().unwrap();
        assert_eq!(config.base_domain, "xn--bcher-kva.example");
        config.base_domain = "a-b.example.com".to_owned();
        config.validate().unwrap();
        for name in [
            "com",
            "co.uk",
            "localhost",
            "127.0.0.1",
            "https://example.com",
            "bad_name.example.com",
            "-bad.example.com",
            "bad-.example.com",
            "bad..example.com",
        ] {
            config.base_domain = name.to_owned();
            assert!(
                config.normalize().and_then(|()| config.validate()).is_err(),
                "accepted invalid base domain: {name}"
            );
        }
        config.base_domain = format!(
            "{}.{}.{}.{}.example.com",
            "a".repeat(55),
            "b".repeat(55),
            "c".repeat(55),
            "d".repeat(55)
        );
        assert!(config.validate().is_err());
        config.base_domain = "example.com".to_owned();
        let ingress_ipv4 = std::mem::take(&mut config.ingress_ipv4);
        let ingress_ipv6 = std::mem::take(&mut config.ingress_ipv6);
        assert!(config.validate().is_err());
        config.ingress_ipv4 = ingress_ipv4;
        config.ingress_ipv6 = ingress_ipv6;
        let https = config.https_listen;
        config.https_listen.set_port(0);
        assert!(config.validate().is_err());
        config.https_listen = https;
        config.proxy_protocol_from = vec!["0.0.0.0/0".parse().unwrap()];
        assert!(config.validate().is_err());
        config.proxy_protocol_from.clear();
        config.challenge_dns_listen = "127.0.0.1:8443".parse().unwrap();
        assert!(config.validate().is_err());
        config.challenge_dns_listen = "0.0.0.0:8053".parse().unwrap();
        config.caddy.push(config.caddy[0].clone());
        assert!(config.validate().is_err());
        config.caddy = vec![config.caddy[0].clone(); 17];
        assert!(config.validate().is_err());
    }
}
