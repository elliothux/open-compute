//! Fresh 256-bit internal tokens and non-secret fingerprints.

use crate::digest::{TOKEN_HEX_LEN, validate_token};
use open_compute_core::{PlatformError, SecretString};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct ActiveGeneration {
    token: SecretString,
    claimed_generation: Option<String>,
}

/// Generation-scoped internal authentication shared with a loopback service.
#[derive(Clone, Default)]
pub struct GenerationAuthRegistry {
    active: Arc<Mutex<Option<ActiveGeneration>>>,
}

/// Opaque credential for platform-owned calls into the current workerd generation.
///
/// Its formatting and serialization surface never exposes the token. Callers should
/// borrow it only while constructing a loopback request.
#[derive(Clone)]
pub struct GenerationCredential(SecretString);

impl GenerationCredential {
    /// Borrow the raw token for an immediate loopback authentication header.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl std::fmt::Debug for GenerationCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GenerationCredential([REDACTED])")
    }
}

impl std::fmt::Debug for GenerationAuthRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerationAuthRegistry")
            .field("active", &self.active_fingerprint())
            .finish()
    }
}

impl GenerationAuthRegistry {
    /// Construct an empty registry that rejects every request.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn activate(&self, token: SecretString) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ActiveGeneration {
            token,
            claimed_generation: None,
        });
    }

    /// Activate a known credential for private-service integration tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn activate_for_test(&self, token: SecretString) {
        self.activate(token);
    }

    pub(crate) fn clear(&self) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Authenticate a token and bind its first bounded process-generation claim.
    /// Subsequent requests must present the same claim.
    pub fn authorize(&self, token: &str, generation: &str) -> bool {
        self.with_authorized(token, generation, || ()).is_some()
    }

    /// Execute a short synchronous authority transaction while generation rotation is fenced.
    /// The callback must not await or re-enter this registry.
    pub fn with_authorized<T>(
        &self,
        token: &str,
        generation: &str,
        operation: impl FnOnce() -> T,
    ) -> Option<T> {
        if generation.is_empty()
            || generation.len() > 128
            || generation.bytes().any(|byte| byte.is_ascii_control())
        {
            return None;
        }
        let mut guard = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = guard.as_mut()?;
        if !constant_time_equal(token.as_bytes(), active.token.expose().as_bytes()) {
            return None;
        }
        match &active.claimed_generation {
            Some(claimed) if !constant_time_equal(generation.as_bytes(), claimed.as_bytes()) => {
                return None;
            }
            Some(_) => {}
            None => {
                active.claimed_generation = Some(generation.to_owned());
            }
        }
        Some(operation())
    }

    /// Commit a host-owned response only while its exact request credential is still current.
    /// The callback must be synchronous, bounded, and must not re-enter this registry.
    pub fn with_current<T>(
        &self,
        credential: &GenerationCredential,
        operation: impl FnOnce() -> T,
    ) -> Option<T> {
        let guard = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = guard.as_ref()?;
        if !constant_time_equal(
            active.token.expose().as_bytes(),
            credential.expose().as_bytes(),
        ) {
            return None;
        }
        Some(operation())
    }

    /// Non-secret active token fingerprint for tests and diagnostics.
    #[must_use]
    pub fn active_fingerprint(&self) -> Option<String> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|active| token_fingerprint(&active.token))
    }

    /// Return the bounded process-generation claim for integration Gate assertions.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn claimed_generation_for_test(&self) -> Option<String> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|active| active.claimed_generation.clone())
    }

    /// Snapshot the active credential for one platform-owned loopback request.
    /// A concurrent generation change may make the credential fail closed.
    #[must_use]
    pub fn credential(&self) -> Option<GenerationCredential> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|active| GenerationCredential(active.token.clone()))
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..len {
        diff |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    diff == 0
}

/// Generate a cryptographically random 256-bit lowercase hex token.
pub fn generate_internal_token() -> Result<SecretString, PlatformError> {
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    let token = SecretString::new(hex::encode(bytes));
    validate_token(&token)?;
    debug_assert_eq!(token.expose().len(), TOKEN_HEX_LEN);
    Ok(token)
}

/// Non-secret uniqueness proof: truncated SHA-256 of the token bytes.
#[must_use]
pub fn token_fingerprint(token: &SecretString) -> String {
    let digest = Sha256::digest(token.expose().as_bytes());
    hex::encode(&digest[..8])
}
