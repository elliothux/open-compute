//! Exact-value redaction that preserves unregistered safe reason codes.

use crate::secret::{SecretBytes, SecretString};

/// Removes registered secret/token values from strings and bytes.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    secrets: Vec<Vec<u8>>,
}

impl Redactor {
    /// Empty redactor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a UTF-8 secret. Empty values are ignored.
    ///
    /// Values that happen to equal a stable reason/error token are still
    /// registered; once registered they are always redacted.
    pub fn register_str(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.register_bytes(value.as_bytes());
    }

    /// Register a [`SecretString`].
    pub fn register_secret_string(&mut self, secret: &SecretString) {
        self.register_str(secret.expose());
    }

    /// Register a [`SecretBytes`] value.
    pub fn register_secret_bytes(&mut self, secret: &SecretBytes) {
        self.register_bytes(secret.expose());
    }

    /// Register raw bytes. Empty values are ignored.
    pub fn register_bytes(&mut self, value: &[u8]) {
        if value.is_empty() {
            return;
        }
        if !self.secrets.iter().any(|existing| existing == value) {
            self.secrets.push(value.to_vec());
        }
        self.secrets
            .sort_by_key(|item| std::cmp::Reverse(item.len()));
    }

    /// Replace registered values in `input` with `[REDACTED]`.
    #[must_use]
    pub fn redact(&self, input: &str) -> String {
        String::from_utf8(self.redact_bytes(input.as_bytes()))
            .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
    }

    /// Replace registered byte sequences with the UTF-8 marker `[REDACTED]`.
    #[must_use]
    pub fn redact_bytes(&self, input: &[u8]) -> Vec<u8> {
        if self.secrets.is_empty() || input.is_empty() {
            return input.to_vec();
        }
        let marker = b"[REDACTED]";
        let mut output = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            if let Some(secret) = self
                .secrets
                .iter()
                .find(|secret| input[i..].starts_with(secret.as_slice()))
            {
                output.extend_from_slice(marker);
                i += secret.len();
            } else {
                output.push(input[i]);
                i += 1;
            }
        }
        output
    }
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
