//! Stable AEAD for secrets.

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, ResourceId, SecretBytes, VersionId, WorkerId,
};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ENVELOPE_VERSION: u8 = 1;
const ALGORITHM: &str = "XCHACHA20-POLY1305";
const NONCE_LEN: usize = 24;
const D1_BOOKMARK_TOKEN_VERSION: u8 = 1;
const D1_BOOKMARK_MAX_BYTES: usize = 256;
/// Current revision-bound secret-envelope AAD schema, independent of artifact format.
pub const SECRET_AAD_SCHEMA: u32 = 1;
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
    r2_cursor_key: [u8; 32],
    vectorize_cursor_key: [u8; 32],
    ai_search_cursor_key: [u8; 32],
    do_name_root_key: [u8; 32],
    do_host_root_key: [u8; 32],
    d1_bookmark_cipher: XChaCha20Poly1305,
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
        let mut r2_cursor_derivation =
            <Hmac<Sha256> as Mac>::new_from_slice(bytes).map_err(|_| {
                PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "failed to derive R2 cursor HMAC key",
                )
            })?;
        r2_cursor_derivation.update(b"open-compute/r2-list-cursor/v1");
        let r2_cursor_key: [u8; 32] = r2_cursor_derivation.finalize().into_bytes().into();
        let vectorize_cursor_key = derive_key(bytes, b"open-compute/vectorize-list-cursor/v1")?;
        let ai_search_cursor_key = derive_key(bytes, b"open-compute/ai-search-list-cursor/v1")?;
        let do_name_root_key = derive_key(bytes, b"open-compute/do-name-root/v1")?;
        let do_host_root_key = derive_key(bytes, b"open-compute/do-host-root/v1")?;
        let d1_bookmark_key = derive_key(bytes, b"open-compute/d1-session-bookmark/v1")?;
        let d1_bookmark_cipher =
            XChaCha20Poly1305::new_from_slice(&d1_bookmark_key).map_err(|_| {
                PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "failed to initialize D1 bookmark AEAD",
                )
            })?;
        Ok(Self {
            cipher,
            key_id: key_id.to_string(),
            fingerprint_key,
            fingerprint_key_id,
            kv_cursor_key,
            r2_cursor_key,
            vectorize_cursor_key,
            ai_search_cursor_key,
            do_name_root_key,
            do_host_root_key,
            d1_bookmark_cipher,
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

    /// Sign an opaque fixed-Wrangler Static Assets token payload.
    #[must_use]
    pub fn sign_asset_upload_token(&self, payload: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.fingerprint_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(b"open-compute/wrangler-assets/v1\0");
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }

    /// Constant-time verification for a fixed-Wrangler Static Assets token.
    #[must_use]
    pub fn verify_asset_upload_token(&self, payload: &[u8], signature: &[u8]) -> bool {
        let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(&self.fingerprint_key) else {
            return false;
        };
        mac.update(b"open-compute/wrangler-assets/v1\0");
        mac.update(payload);
        mac.verify_slice(signature).is_ok()
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

    /// Sign a canonical R2 list-cursor payload with an independent key.
    #[must_use]
    pub fn sign_r2_cursor(&self, payload: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.r2_cursor_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }

    /// Constant-time verification of a canonical R2 list-cursor payload.
    #[must_use]
    pub fn verify_r2_cursor(&self, payload: &[u8], signature: &[u8]) -> bool {
        let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(&self.r2_cursor_key) else {
            return false;
        };
        mac.update(payload);
        mac.verify_slice(signature).is_ok()
    }

    /// Sign a canonical Vectorize list-cursor payload with an independent key.
    #[must_use]
    pub fn sign_vectorize_cursor(&self, payload: &[u8]) -> [u8; 32] {
        sign_cursor(&self.vectorize_cursor_key, payload)
    }

    /// Constant-time verification of a canonical Vectorize list cursor.
    #[must_use]
    pub fn verify_vectorize_cursor(&self, payload: &[u8], signature: &[u8]) -> bool {
        verify_cursor(&self.vectorize_cursor_key, payload, signature)
    }

    /// Sign a canonical AI Search list-cursor payload with an independent key.
    #[must_use]
    pub fn sign_ai_search_cursor(&self, payload: &[u8]) -> [u8; 32] {
        sign_cursor(&self.ai_search_cursor_key, payload)
    }

    /// Constant-time verification of a canonical AI Search list cursor.
    #[must_use]
    pub fn verify_ai_search_cursor(&self, payload: &[u8], signature: &[u8]) -> bool {
        verify_cursor(&self.ai_search_cursor_key, payload, signature)
    }

    /// Seal an opaque D1 session bookmark bound to one database and state version.
    pub fn seal_d1_bookmark(
        &self,
        account: AccountId,
        resource: ResourceId,
        session_version: u64,
    ) -> Result<String, PlatformError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::D1SessionError,
                    "failed to generate D1 bookmark nonce",
                )
            })?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = d1_bookmark_aad(account, resource);
        let ciphertext = self
            .d1_bookmark_cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &session_version.to_be_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| {
                PlatformError::new(ErrorCode::D1SessionError, "D1 bookmark sealing failed")
            })?;
        let mut token = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        token.push(D1_BOOKMARK_TOKEN_VERSION);
        token.extend_from_slice(&nonce_bytes);
        token.extend_from_slice(&ciphertext);
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token))
    }

    /// Open a D1 session bookmark and return its sealed database state version.
    pub fn open_d1_bookmark(
        &self,
        account: AccountId,
        resource: ResourceId,
        token: &str,
    ) -> Result<u64, PlatformError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| session_bookmark_error())?;
        if bytes.len() <= 1 + NONCE_LEN || bytes.len() > D1_BOOKMARK_MAX_BYTES {
            return Err(session_bookmark_error());
        }
        if bytes[0] != D1_BOOKMARK_TOKEN_VERSION {
            return Err(session_bookmark_error());
        }
        let nonce = XNonce::from_slice(&bytes[1..1 + NONCE_LEN]);
        let aad = d1_bookmark_aad(account, resource);
        let plaintext = self
            .d1_bookmark_cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &bytes[1 + NONCE_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| session_bookmark_error())?;
        let version =
            <[u8; 8]>::try_from(plaintext.as_slice()).map_err(|_| session_bookmark_error())?;
        Ok(u64::from_be_bytes(version))
    }

    /// Derive the namespace-local HMAC key injected only into the tenant facade closure.
    #[must_use]
    pub fn durable_object_name_key(&self, namespace_storage_key: &str) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.do_name_root_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(b"open-compute/do-namespace-name/v1\0");
        mac.update(namespace_storage_key.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Derive the opaque native host-actor name for one object generation.
    #[must_use]
    pub fn durable_object_host_key(
        &self,
        namespace_storage_key: &str,
        object_id: &str,
        object_generation: u64,
    ) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.do_host_root_key)
            .expect("SHA-256 HMAC accepts a 32-byte key");
        mac.update(b"oc-do-host-v1\0");
        mac.update(namespace_storage_key.as_bytes());
        mac.update(b"\0");
        mac.update(object_id.as_bytes());
        mac.update(&object_generation.to_be_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }

    /// Encrypt a version secret bound to its immutable revision and identity.
    pub fn encrypt(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        worker: WorkerId,
        version: VersionId,
        secret_name: &str,
        revision_id: &str,
    ) -> Result<SecretEnvelope, PlatformError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to generate AEAD nonce")
            })?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = associated_data(account, worker, version, secret_name, revision_id)?;
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

    /// Decrypt a version secret, rejecting envelope, revision, or identity mismatch.
    pub fn decrypt(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        worker: WorkerId,
        version: VersionId,
        secret_name: &str,
        revision_id: &str,
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
        let aad = associated_data(account, worker, version, secret_name, revision_id)?;
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

    /// Seal one R2 SSE-C key bound to its account, bucket, and tenant upload id.
    pub fn encrypt_r2_ssec(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        resource: ResourceId,
        upload_id: &str,
    ) -> Result<SecretEnvelope, PlatformError> {
        if plaintext.expose().len() != 32 {
            return Err(PlatformError::new(
                ErrorCode::R2SsecInvalid,
                "R2 SSE-C key is invalid or does not match the object",
            ));
        }
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to generate AEAD nonce")
            })?;
        let nonce = XNonce::from(nonce_bytes);
        let aad = r2_ssec_aad(account, resource, upload_id)?;
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

    /// Seal one R2 object SSE-C key to its committed object version.
    pub fn encrypt_r2_object_ssec(
        &self,
        plaintext: &SecretBytes,
        account: AccountId,
        resource: ResourceId,
        object_version: &str,
    ) -> Result<SecretEnvelope, PlatformError> {
        self.encrypt_r2_ssec(
            plaintext,
            account,
            resource,
            &format!("object/{object_version}"),
        )
    }

    /// Open a sealed R2 SSE-C key, rejecting identity or envelope mismatch.
    pub fn decrypt_r2_ssec(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        resource: ResourceId,
        upload_id: &str,
    ) -> Result<SecretBytes, PlatformError> {
        if envelope.version != ENVELOPE_VERSION
            || envelope.algorithm != ALGORITHM
            || envelope.nonce.len() != NONCE_LEN
        {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "R2 SSE-C envelope is invalid",
            ));
        }
        if envelope.key_id != self.key_id {
            return Err(PlatformError::new(
                ErrorCode::MasterKeyMismatch,
                "secret envelope key id mismatch",
            ));
        }
        let nonce = XNonce::from_slice(&envelope.nonce);
        let aad = r2_ssec_aad(account, resource, upload_id)?;
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
                PlatformError::new(
                    ErrorCode::ResourceInvariantViolation,
                    "R2 SSE-C envelope is invalid",
                )
            })?;
        if plaintext.len() != 32 {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "R2 SSE-C envelope is invalid",
            ));
        }
        Ok(SecretBytes::new(plaintext))
    }

    /// Open an R2 object SSE-C key sealed to its committed object version.
    pub fn decrypt_r2_object_ssec(
        &self,
        envelope: &SecretEnvelope,
        account: AccountId,
        resource: ResourceId,
        object_version: &str,
    ) -> Result<SecretBytes, PlatformError> {
        self.decrypt_r2_ssec(
            envelope,
            account,
            resource,
            &format!("object/{object_version}"),
        )
    }
}

fn sign_cursor(key: &[u8; 32], payload: &[u8]) -> [u8; 32] {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("SHA-256 HMAC accepts a 32-byte key");
    mac.update(payload);
    mac.finalize().into_bytes().into()
}

fn verify_cursor(key: &[u8; 32], payload: &[u8], signature: &[u8]) -> bool {
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(key) else {
        return false;
    };
    mac.update(payload);
    mac.verify_slice(signature).is_ok()
}

fn r2_ssec_aad(
    account: AccountId,
    resource: ResourceId,
    upload_id: &str,
) -> Result<Vec<u8>, PlatformError> {
    if upload_id.is_empty() || upload_id.len() > MAX_SECRET_NAME_LEN {
        return Err(PlatformError::new(
            ErrorCode::R2MultipartInvalid,
            "R2 multipart upload is invalid",
        ));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&SECRET_AAD_SCHEMA.to_be_bytes());
    out.extend_from_slice(b"open-compute/r2-multipart-ssec/v1");
    write_framed(&mut out, account.as_canonical_str().as_bytes())?;
    write_framed(&mut out, resource.as_canonical_str().as_bytes())?;
    write_framed(&mut out, upload_id.as_bytes())?;
    Ok(out)
}

fn d1_bookmark_aad(account: AccountId, resource: ResourceId) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(account.as_canonical_str().as_bytes());
    out.push(0);
    out.extend_from_slice(resource.as_canonical_str().as_bytes());
    out
}

fn session_bookmark_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1SessionError,
        "D1 session bookmark is invalid for this database",
    )
}

fn derive_key(master: &[u8], domain: &[u8]) -> Result<[u8; 32], PlatformError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(master).map_err(|_| {
        PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "failed to derive Durable Object HMAC key",
        )
    })?;
    mac.update(domain);
    Ok(mac.finalize().into_bytes().into())
}

fn associated_data(
    account: AccountId,
    worker: WorkerId,
    version: VersionId,
    secret_name: &str,
    revision_id: &str,
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
    out.extend_from_slice(&SECRET_AAD_SCHEMA.to_be_bytes());
    write_framed(&mut out, account.as_canonical_str().as_bytes())?;
    write_framed(&mut out, worker.as_canonical_str().as_bytes())?;
    write_framed(&mut out, version.as_canonical_str().as_bytes())?;
    write_framed(&mut out, secret_name.as_bytes())?;
    if revision_id.is_empty() || revision_id.len() > MAX_SECRET_NAME_LEN {
        return Err(PlatformError::new(
            ErrorCode::SecretInvalid,
            "secret revision is invalid",
        ));
    }
    write_framed(&mut out, revision_id.as_bytes())?;
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

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
