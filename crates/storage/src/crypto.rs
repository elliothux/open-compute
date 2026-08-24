//! Stable AEAD for secrets.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use open_compute_core::{AccountId, DeploymentId, ErrorCode, PlatformError, SecretBytes, WorkerId};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ENVELOPE_VERSION: u8 = 1;
const ALGORITHM: &str = "XCHACHA20-POLY1305";
const NONCE_LEN: usize = 24;
/// Explicit secret-envelope AAD schema, independent of artifact schema version.
pub const SECRET_AAD_SCHEMA: u32 = 1;
/// Revision-bound secret-envelope AAD schema used by immutable deployments.
pub const SECRET_AAD_REVISION_SCHEMA: u32 = 2;
const MAX_SECRET_NAME_LEN: usize = 4096;
const KEY_ID_LEN: usize = 64;

/// Serializable ciphertext envelope. Contains no plaintext.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretEnvelope {
    /// Envelope format version.
    pub version: u8,
    /// Master-key fingerprint used at encryption time.
    pub key_id: String,
    /// AEAD algorithm identifier.
    pub algorithm: String,
    /// 24-byte nonce for XChaCha20-Poly1305.
    pub nonce: Vec<u8>,
    /// Ciphertext and Poly1305 tag.
    pub ciphertext: Vec<u8>,
}

/// Encrypts and decrypts secrets with a resolved master key.
pub struct SecretCrypto {
    cipher: XChaCha20Poly1305,
    key_id: String,
    fingerprint_key: [u8; 32],
    fingerprint_key_id: String,
    kv_cursor_key: [u8; 32],
}

impl std::fmt::Debug for SecretCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretCrypto")
            .field("key_id", &self.key_id)
            .field("algorithm", &ALGORITHM)
            .finish()
    }
}

impl SecretCrypto {
    /// Build crypto from a 32-byte master key and its fingerprint.
    pub fn new(key: &SecretBytes, key_id: &str) -> Result<Self, PlatformError> {
        let bytes = key.expose();
        if bytes.len() != 32 {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "master key for AEAD must be 32 bytes",
            ));
        }
        if !is_sha256_fingerprint(key_id) {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "master key id must be a 64-character lowercase SHA-256 fingerprint",
            ));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(bytes).map_err(|_| {
            PlatformError::new(ErrorCode::MasterKeyMismatch, "failed to initialize AEAD")
        })?;
        let mut derivation = <Hmac<Sha256> as Mac>::new_from_slice(bytes).map_err(|_| {
            PlatformError::new(ErrorCode::MasterKeyMismatch, "failed to derive HMAC key")
        })?;
        derivation.update(b"open-compute/control-idempotency/v1");
        let fingerprint_key: [u8; 32] = derivation.finalize().into_bytes().into();
        let fingerprint_key_id = hex::encode(Sha256::digest(fingerprint_key));
        let mut cursor_derivation = <Hmac<Sha256> as Mac>::new_from_slice(bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "failed to derive cursor HMAC key",
            )
        })?;
        cursor_derivation.update(b"open-compute/kv-list-cursor/v1");
        let kv_cursor_key: [u8; 32] = cursor_derivation.finalize().into_bytes().into();
        Ok(Self {
            cipher,
            key_id: key_id.to_string(),
            fingerprint_key,
            fingerprint_key_id,
            kv_cursor_key,
        })
    }

    /// Master-key fingerprint.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Identifier for the independently derived idempotency HMAC key.
    #[must_use]
    pub fn fingerprint_key_id(&self) -> &str {
        &self.fingerprint_key_id
    }

    /// HMAC a canonical control request without persisting low-entropy secret hashes.
    #[must_use]
    pub fn fingerprint_request(&self, canonical_request: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.fingerprint_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(b"open-compute/control-request/v1\0");
        mac.update(canonical_request);
        mac.finalize().into_bytes().into()
    }

    /// Sign a canonical KV list-cursor payload with a domain-separated key.
    #[must_use]
    pub fn sign_kv_cursor(&self, payload: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.kv_cursor_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }

    /// Constant-time verification of a canonical KV list-cursor payload.
    #[must_use]
    pub fn verify_kv_cursor(&self, payload: &[u8], signature: &[u8]) -> bool {
        let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(&self.kv_cursor_key) else {
            return false;
        };
        mac.update(payload);
        mac.verify_slice(signature).is_ok()
    }

    /// Encrypt `plaintext` bound to the canonical associated-data context.
    pub fn encrypt(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
    ) -> Result<SecretEnvelope, PlatformError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to generate AEAD nonce")
            })?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = associated_data(account, worker, deployment, secret_name, None)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.expose(),
                    aad: &aad,
                },
            )
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "secret encryption failed")
            })?;
        Ok(SecretEnvelope {
            version: ENVELOPE_VERSION,
            key_id: self.key_id.clone(),
            algorithm: ALGORITHM.to_string(),
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    /// Encrypt a deployment secret with its immutable random revision in AAD.
    pub fn encrypt_revision(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
        revision_id: &str,
    ) -> Result<SecretEnvelope, PlatformError> {
        self.encrypt_inner(
            plaintext,
            account,
            worker,
            deployment,
            secret_name,
            Some(revision_id),
        )
    }

    fn encrypt_inner(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
        revision_id: Option<&str>,
    ) -> Result<SecretEnvelope, PlatformError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to generate AEAD nonce")
            })?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = associated_data(account, worker, deployment, secret_name, revision_id)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.expose(),
                    aad: &aad,
                },
            )
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "secret encryption failed")
            })?;
        Ok(SecretEnvelope {
            version: ENVELOPE_VERSION,
            key_id: self.key_id.clone(),
            algorithm: ALGORITHM.to_string(),
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    /// Decrypt `envelope`, rejecting version/algorithm/key/context mismatch and tampering.
    pub fn decrypt(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
    ) -> Result<SecretBytes, PlatformError> {
        if envelope.version != ENVELOPE_VERSION {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope version is unsupported",
            ));
        }
        if envelope.algorithm != ALGORITHM {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope algorithm mismatch",
            ));
        }
        if envelope.key_id != self.key_id {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "secret envelope key id mismatch",
            ));
        }
        if envelope.nonce.len() != NONCE_LEN {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope nonce is invalid",
            ));
        }
        let nonce = XNonce::from_slice(&envelope.nonce);
        let aad = associated_data(account, worker, deployment, secret_name, None)?;
        let plaintext = self
            .cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &envelope.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "secret decryption failed")
            })?;
        Ok(SecretBytes::new(plaintext))
    }

    /// Decrypt a deployment secret while authenticating its immutable revision.
    pub fn decrypt_revision(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
        revision_id: &str,
    ) -> Result<SecretBytes, PlatformError> {
        self.decrypt_inner(
            envelope,
            account,
            worker,
            deployment,
            secret_name,
            Some(revision_id),
        )
    }

    fn decrypt_inner(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        worker: WorkerId,
        deployment: DeploymentId,
        secret_name: &str,
        revision_id: Option<&str>,
    ) -> Result<SecretBytes, PlatformError> {
        if envelope.version != ENVELOPE_VERSION {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope version is unsupported",
            ));
        }
        if envelope.algorithm != ALGORITHM {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope algorithm mismatch",
            ));
        }
        if envelope.key_id != self.key_id {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "secret envelope key id mismatch",
            ));
        }
        if envelope.nonce.len() != NONCE_LEN {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "secret envelope nonce is invalid",
            ));
        }
        let nonce = XNonce::from_slice(&envelope.nonce);
        let aad = associated_data(account, worker, deployment, secret_name, revision_id)?;
        let plaintext = self
            .cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &envelope.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "secret decryption failed")
            })?;
        Ok(SecretBytes::new(plaintext))
    }
}

fn associated_data(
    account: AccountId,
    worker: WorkerId,
    deployment: DeploymentId,
    secret_name: &str,
    revision_id: Option<&str>,
) -> Result<Vec<u8>, PlatformError> {
    if secret_name.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "secret name must not be empty",
        ));
    }
    if secret_name.len() > MAX_SECRET_NAME_LEN {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "secret name exceeds the maximum framed length",
        ));
    }
    let mut out = Vec::new();
    out.extend_from_slice(
        &revision_id
            .map_or(SECRET_AAD_SCHEMA, |_| SECRET_AAD_REVISION_SCHEMA)
            .to_be_bytes(),
    );
    write_framed(&mut out, account.as_canonical_str().as_bytes())?;
    write_framed(&mut out, worker.as_canonical_str().as_bytes())?;
    write_framed(&mut out, deployment.as_canonical_str().as_bytes())?;
    write_framed(&mut out, secret_name.as_bytes())?;
    if let Some(revision) = revision_id {
        if revision.is_empty() || revision.len() > MAX_SECRET_NAME_LEN {
            return Err(PlatformError::new(
                ErrorCode::SecretInvalid,
                "secret revision is invalid",
            ));
        }
        write_framed(&mut out, revision.as_bytes())?;
    }
    Ok(out)
}

fn write_framed(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), PlatformError> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "associated-data field exceeds u32 length",
        )
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn is_sha256_fingerprint(key_id: &str) -> bool {
    key_id.len() == KEY_ID_LEN
        && key_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
