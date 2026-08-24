//! Secret-bearing wrappers that never print or serialize their contents.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Debug, Display, Formatter};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const REDACTED: &str = "[REDACTED]";

/// UTF-8 secret. Debug, Display, and Serialize always emit `[REDACTED]`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretString {
    value: String,
}

impl SecretString {
    /// Wrap a secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Borrow the secret for a short, documented use site.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.value
    }
}

impl Debug for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

impl Display for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

/// Binary secret. Debug, Display, and Serialize always emit `[REDACTED]`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretBytes {
    value: Zeroizing<Vec<u8>>,
}

impl SecretBytes {
    /// Wrap secret bytes.
    #[must_use]
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self {
            value: Zeroizing::new(value.into()),
        }
    }

    /// Borrow the secret bytes for a short, documented use site.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.value
    }
}

impl Debug for SecretBytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}

impl Display for SecretBytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl Serialize for SecretBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

#[cfg(test)]
#[path = "secret_tests.rs"]
mod tests;
