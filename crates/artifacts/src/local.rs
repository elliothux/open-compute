//! Secure fd-relative local object authority.

use crate::backend::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ListPage, ListedObject, ObjectBody,
    ObjectGet, ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, PutMode, PutOptions,
    UploadedPart,
};
use base64::Engine as _;
use bytes::Bytes;
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use md5::{Digest as _, Md5};
use open_compute_core::{ErrorCode, LocalObjectStorageConfig, PlatformError, PlatformId};
use rand::RngCore as _;
use rustix::fd::{AsFd as _, OwnedFd};
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, RenameFlags, fchmod, flock, fstat, fsync, mkdirat, open,
    openat, renameat, renameat_with, statat, unlinkat,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek as _, SeekFrom, Write as _};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::str::FromStr as _;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

const FORMAT_SCHEMA: u32 = 1;
const HEADER_BYTES: usize = 64 * 1024;
const MAGIC: &[u8; 8] = b"OCOBJ001";
const CHUNK_BYTES: usize = 64 * 1024;
const AEAD_TAG_BYTES: usize = 16;
const MAX_SCAN_ENTRIES: usize = 1_000_000;
const MAX_SCAN_BYTES: u64 = 1 << 40;
const MAX_SCAN_DURATION: Duration = Duration::from_secs(30);
const OBJECT_FILE: &str = "object.ocobj";
const FORMAT_FILE: &str = "format.json";
const LOCK_FILE: &str = "backend.lock";
const OBJECTS_DIR: &str = "objects";
const MULTIPART_DIR: &str = "multipart";
const MANIFEST_FILE: &str = "manifest.json";
const PARTS_DIR: &str = "parts";
const CURSOR_PREFIX: &str = "local-v1:";

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub(crate) enum LocalFaultPoint {
    BeforeEnvelopeFsync = 1,
    AfterEnvelopeFsync = 2,
    BeforePublishRename = 3,
    AfterPublishRename = 4,
    AfterDeleteUnlink = 5,
    MultipartIntentCommitted = 6,
    MultipartBeforePublish = 7,
    MultipartAfterPublish = 8,
    MultipartBeforeRetire = 9,
    MultipartAbortIntent = 10,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FormatMarker {
    schema_version: u32,
    platform_id: String,
    root_id: String,
    prefix: String,
    r2_prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvelopeHeader {
    schema_version: u32,
    key_sha256: String,
    size: u64,
    stored_size: u64,
    etag: String,
    last_modified_ms: i64,
    payload_sha256: String,
    metadata: ObjectMetadata,
    encryption: Option<EncryptionHeader>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptionHeader {
    algorithm: String,
    chunk_size: u32,
    object_version: String,
    nonce: String,
    verifier: String,
    ssec_key_md5: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeaderRecord {
    header: EnvelopeHeader,
    header_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MultipartManifest {
    schema_version: u32,
    upload_id: String,
    key: ObjectKey,
    metadata: ObjectMetadata,
    encryption: Option<EncryptionHeader>,
    created_at_ms: i64,
    status: MultipartStatus,
}

struct ScanBudget {
    entries: usize,
    bytes: u64,
    started: Instant,
}

impl ScanBudget {
    fn new() -> Self {
        Self {
            entries: 0,
            bytes: 0,
            started: Instant::now(),
        }
    }

    fn charge(&mut self, bytes: u64) -> Result<(), BackendError> {
        self.entries = self.entries.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.entries > MAX_SCAN_ENTRIES
            || self.bytes > MAX_SCAN_BYTES
            || self.started.elapsed() > MAX_SCAN_DURATION
        {
            return Err(BackendError::Capacity);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum MultipartStatus {
    Uploading,
    Publishing { etag: String },
    Aborting,
}

/// One exclusively locked local object authority.
#[derive(Clone)]
pub(crate) struct LocalBackend {
    root: Arc<OwnedFd>,
    _lock: Arc<File>,
    prefix: Arc<str>,
    r2_prefix: Arc<str>,
    authority_sha256: [u8; 32],
    max_object_bytes: u64,
    free_space_hard_bytes: u64,
    partial_grace_ms: u64,
    key_locks: Arc<Vec<Mutex<()>>>,
    #[cfg(test)]
    fault: Arc<AtomicU8>,
}

mod backend;
mod codec;
mod envelope;
mod filesystem;
mod fs_ops;
mod multipart;

use codec::*;
use envelope::*;
use filesystem::*;
use fs_ops::*;
use multipart::*;
