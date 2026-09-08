//! Validated values used by the per-user Wrangler target registry.

use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use url::{Host, Url};

/// Maximum byte length of a remote target name.
pub const TARGET_NAME_MAX_BYTES: usize = 32;

/// A lowercase developer-chosen remote target alias.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TargetName(String);

impl TargetName {
    /// Validated target name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for TargetName {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = value.bytes();
        let valid = value.len() <= TARGET_NAME_MAX_BYTES
            && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(PlatformError::new(
                ErrorCode::TargetInvalid,
                "target name must match [a-z][a-z0-9-]{0,31}",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl Display for TargetName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for TargetName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TargetName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A canonical lowercase 32-hex Cloudflare-compatible public account identifier.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct CloudflareAccountId(String);

impl CloudflareAccountId {
    /// Validated account identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CloudflareAccountId {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(PlatformError::new(
                ErrorCode::TargetInvalid,
                "target account ID must be canonical lowercase 32-hex",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl Display for CloudflareAccountId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for CloudflareAccountId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CloudflareAccountId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A normalized open-compute Cloudflare API base URL ending in `/client/v4`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TargetApiBaseUrl(String);

impl TargetApiBaseUrl {
    /// Canonical URL text without a trailing slash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Origin-only URL for human-safe target summaries.
    #[must_use]
    pub fn origin(&self) -> &str {
        self.0.strip_suffix("/client/v4").unwrap_or(&self.0)
    }

    /// Join one API-relative path without changing origin.
    #[must_use]
    pub fn endpoint(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.0)
    }
}

impl FromStr for TargetApiBaseUrl {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut url = Url::parse(value).map_err(|_| target_url_invalid())?;
        if url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.host().is_none()
        {
            return Err(target_url_invalid());
        }
        if !matches!(url.path(), "/client/v4" | "/client/v4/") {
            return Err(target_url_invalid());
        }
        match url.scheme() {
            "https" => {}
            "http" if loopback_host(&url) => {}
            _ => {
                return Err(PlatformError::new(
                    ErrorCode::TargetInvalid,
                    "target API URL must use HTTPS except for loopback",
                ));
            }
        }
        url.set_path("/client/v4");
        Ok(Self(url.to_string().trim_end_matches('/').to_owned()))
    }
}

impl Display for TargetApiBaseUrl {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for TargetApiBaseUrl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TargetApiBaseUrl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

fn loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    }
}

fn target_url_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::TargetInvalid,
        "target API URL must be an origin followed by /client/v4",
    )
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
