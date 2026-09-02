//! Canonical, length-framed `WorkerBundleV1` encoding.

use open_compute_core::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const MAGIC: &[u8; 8] = b"OCWB\0\x01\0\0";
const HEADER_BYTES: usize = 12;

/// Canonical artifact schema version.
pub const WORKER_BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Structural version bundle limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BundleLimits {
    /// Maximum canonical manifest bytes.
    pub max_manifest_bytes: usize,
    /// Maximum module count.
    pub max_modules: usize,
    /// Maximum bytes in one module.
    pub max_module_bytes: usize,
    /// Maximum raw module bytes across the bundle.
    pub max_total_module_bytes: usize,
    /// Maximum complete canonical artifact bytes.
    pub max_artifact_bytes: usize,
}

impl Default for BundleLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 256 * 1024,
            max_modules: 128,
            max_module_bytes: 4 * 1024 * 1024,
            max_total_module_bytes: 16 * 1024 * 1024,
            max_artifact_bytes: 17 * 1024 * 1024,
        }
    }
}

/// Supported module representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModuleType {
    /// JavaScript ES module.
    EsModule,
    /// `CommonJS` module gated by compatibility policy.
    CommonJsModule,
    /// UTF-8 text module.
    Text,
    /// Arbitrary data module.
    Data,
    /// JSON module.
    Json,
    /// WebAssembly module.
    Wasm,
}

/// Caller-provided module before canonicalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleInput {
    /// Logical module name, never a filesystem path.
    pub name: String,
    /// Module kind.
    pub module_type: ModuleType,
    /// Raw module bytes.
    pub bytes: Vec<u8>,
}

/// Canonical module descriptor stored in the manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleManifest {
    /// Canonical logical name.
    pub name: String,
    /// Module kind.
    #[serde(rename = "type")]
    pub module_type: ModuleType,
    /// Lowercase SHA-256.
    pub sha256: String,
    /// Raw byte length.
    pub size: u64,
    /// Offset from the first byte after the manifest.
    pub offset: u64,
}

/// Canonical `WorkerBundleV1` manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerBundleManifest {
    /// Framing schema version.
    pub schema_version: u32,
    /// Main module name.
    pub main_module: String,
    /// Byte-wise sorted modules.
    pub modules: Vec<ModuleManifest>,
}

/// Verified canonical artifact and manifest.
#[derive(Clone, Eq, PartialEq)]
pub struct CanonicalBundle {
    bytes: Vec<u8>,
    manifest: WorkerBundleManifest,
    sha256: [u8; 32],
    blob_offset: usize,
}

/// Verified canonical bundle retained in a private staging file.
///
/// The path is intentionally omitted from `Debug`; data-directory topology is
/// not part of a control-plane error or log contract.
#[derive(Clone, Eq, PartialEq)]
pub struct StagedBundle {
    path: PathBuf,
    manifest: WorkerBundleManifest,
    sha256: [u8; 32],
    size: u64,
}

impl std::fmt::Debug for StagedBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedBundle")
            .field("sha256", &hex::encode(self.sha256))
            .field("size", &self.size)
            .field("manifest", &self.manifest)
            .finish()
    }
}

impl StagedBundle {
    /// Open and incrementally verify a canonical `WorkerBundleV1` file.
    pub fn open(path: PathBuf, limits: BundleLimits) -> Result<Self, PlatformError> {
        let file = File::open(&path).map_err(|_| invalid("version staging file is unavailable"))?;
        let metadata = file
            .metadata()
            .map_err(|_| invalid("version staging file is unavailable"))?;
        if !metadata.file_type().is_file() {
            return Err(invalid("version staging path is not a regular file"));
        }
        let size = metadata.len();
        if size > u64::try_from(limits.max_artifact_bytes).unwrap_or(u64::MAX) {
            return Err(too_large());
        }

        let mut reader = BufReader::new(file);
        let mut header = [0_u8; HEADER_BYTES];
        read_exact_bundle(&mut reader, &mut header, "bundle header is truncated")?;
        if &header[..8] != MAGIC {
            return Err(invalid("bundle magic or version is invalid"));
        }
        let manifest_len = u32::from_be_bytes(
            header[8..12]
                .try_into()
                .map_err(|_| invalid("bundle header is truncated"))?,
        ) as usize;
        if manifest_len == 0 || manifest_len > limits.max_manifest_bytes {
            return Err(too_large_or_invalid(manifest_len == 0));
        }
        let mut manifest_bytes = vec![0_u8; manifest_len];
        read_exact_bundle(
            &mut reader,
            &mut manifest_bytes,
            "bundle manifest is truncated",
        )?;
        let manifest = parse_manifest(&manifest_bytes, limits)?;
        let expected_module_bytes = manifest
            .modules
            .last()
            .and_then(|module| module.offset.checked_add(module.size))
            .ok_or_else(|| invalid("bundle module layout is invalid"))?;
        let expected_size = u64::try_from(HEADER_BYTES)
            .ok()
            .and_then(|value| value.checked_add(u64::try_from(manifest_len).ok()?))
            .and_then(|value| value.checked_add(expected_module_bytes))
            .ok_or_else(too_large)?;
        if expected_size != size {
            return Err(invalid("bundle is truncated or contains trailing bytes"));
        }

        let mut artifact_hasher = Sha256::new();
        artifact_hasher.update(header);
        artifact_hasher.update(&manifest_bytes);
        let mut scratch = vec![0_u8; 64 * 1024];
        for module in &manifest.modules {
            let mut remaining = module.size;
            let mut module_hasher = Sha256::new();
            let needs_text = matches!(module.module_type, ModuleType::Text | ModuleType::Json);
            let mut text = if needs_text {
                Vec::with_capacity(usize::try_from(module.size).map_err(|_| too_large())?)
            } else {
                Vec::new()
            };
            while remaining != 0 {
                let take = scratch
                    .len()
                    .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                let chunk = &mut scratch[..take];
                read_exact_bundle(&mut reader, chunk, "module bytes are truncated")?;
                artifact_hasher.update(&*chunk);
                module_hasher.update(&*chunk);
                if needs_text {
                    text.extend_from_slice(chunk);
                }
                remaining -= u64::try_from(take).map_err(|_| too_large())?;
            }
            if hex::encode(module_hasher.finalize()) != module.sha256 {
                return Err(PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "module digest does not match the canonical manifest",
                ));
            }
            if needs_text && std::str::from_utf8(&text).is_err() {
                return Err(invalid("text and JSON modules must be valid UTF-8"));
            }
            if module.module_type == ModuleType::Json
                && serde_json::from_slice::<serde_json::Value>(&text).is_err()
            {
                return Err(invalid("JSON module is invalid"));
            }
        }
        Ok(Self {
            path,
            manifest,
            sha256: artifact_hasher.finalize().into(),
            size,
        })
    }

    /// Private staging file containing the verified artifact.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Verified canonical manifest.
    #[must_use]
    pub const fn manifest(&self) -> &WorkerBundleManifest {
        &self.manifest
    }

    /// Whole-artifact SHA-256.
    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    /// Whole-artifact byte length.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

impl std::fmt::Debug for CanonicalBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalBundle")
            .field("sha256", &hex::encode(self.sha256))
            .field("size", &self.bytes.len())
            .field("manifest", &self.manifest)
            .finish()
    }
}

impl CanonicalBundle {
    /// Canonicalize modules and produce the unique `WorkerBundleV1` byte representation.
    pub fn build(
        main_module: &str,
        modules: Vec<ModuleInput>,
        limits: BundleLimits,
    ) -> Result<Self, PlatformError> {
        if modules.is_empty() || modules.len() > limits.max_modules {
            return Err(too_large_or_invalid(modules.is_empty()));
        }
        let canonical_main = canonical_module_name(main_module)?;
        let mut normalized = Vec::with_capacity(modules.len());
        let mut total = 0_usize;
        for module in modules {
            let name = canonical_module_name(&module.name)?;
            if module.bytes.len() > limits.max_module_bytes {
                return Err(too_large());
            }
            total = total
                .checked_add(module.bytes.len())
                .ok_or_else(too_large)?;
            if total > limits.max_total_module_bytes {
                return Err(too_large());
            }
            if matches!(module.module_type, ModuleType::Text | ModuleType::Json)
                && std::str::from_utf8(&module.bytes).is_err()
            {
                return Err(invalid("text and JSON modules must be valid UTF-8"));
            }
            if module.module_type == ModuleType::Json
                && serde_json::from_slice::<serde_json::Value>(&module.bytes).is_err()
            {
                return Err(invalid("JSON module is invalid"));
            }
            normalized.push(ModuleInput {
                name,
                module_type: module.module_type,
                bytes: module.bytes,
            });
        }
        normalized.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        let mut names = BTreeSet::new();
        if normalized
            .iter()
            .any(|module| !names.insert(module.name.clone()))
        {
            return Err(invalid("bundle contains duplicate module names"));
        }
        let main = normalized
            .iter()
            .find(|module| module.name == canonical_main)
            .ok_or_else(|| invalid("main module was not found"))?;
        if main.module_type != ModuleType::EsModule {
            return Err(invalid("main module must be an ES module"));
        }

        let mut offset = 0_u64;
        let mut manifest_modules = Vec::with_capacity(normalized.len());
        for module in &normalized {
            let size = u64::try_from(module.bytes.len()).map_err(|_| too_large())?;
            manifest_modules.push(ModuleManifest {
                name: module.name.clone(),
                module_type: module.module_type,
                sha256: hex::encode(Sha256::digest(&module.bytes)),
                size,
                offset,
            });
            offset = offset.checked_add(size).ok_or_else(too_large)?;
        }
        let manifest = WorkerBundleManifest {
            schema_version: WORKER_BUNDLE_SCHEMA_VERSION,
            main_module: canonical_main,
            modules: manifest_modules,
        };
        let manifest_bytes = serde_json::to_vec(&manifest)
            .map_err(|_| invalid("bundle manifest could not be encoded"))?;
        if manifest_bytes.len() > limits.max_manifest_bytes {
            return Err(too_large());
        }
        let total_size = HEADER_BYTES
            .checked_add(manifest_bytes.len())
            .and_then(|n| n.checked_add(total))
            .ok_or_else(too_large)?;
        if total_size > limits.max_artifact_bytes {
            return Err(too_large());
        }
        let manifest_len = u32::try_from(manifest_bytes.len()).map_err(|_| too_large())?;
        let mut bytes = Vec::with_capacity(total_size);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&manifest_len.to_be_bytes());
        bytes.extend_from_slice(&manifest_bytes);
        for module in &normalized {
            bytes.extend_from_slice(&module.bytes);
        }
        Self::parse(bytes, limits)
    }

    /// Parse and verify canonical framing, offsets, sizes, and all digests.
    pub fn parse(bytes: Vec<u8>, limits: BundleLimits) -> Result<Self, PlatformError> {
        if bytes.len() > limits.max_artifact_bytes {
            return Err(too_large());
        }
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(invalid("bundle magic or version is invalid"));
        }
        let manifest_len = u32::from_be_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| invalid("bundle header is truncated"))?,
        ) as usize;
        if manifest_len == 0 || manifest_len > limits.max_manifest_bytes {
            return Err(too_large_or_invalid(manifest_len == 0));
        }
        let blob_offset = HEADER_BYTES
            .checked_add(manifest_len)
            .ok_or_else(too_large)?;
        let manifest_bytes = bytes
            .get(HEADER_BYTES..blob_offset)
            .ok_or_else(|| invalid("bundle manifest is truncated"))?;
        let manifest: WorkerBundleManifest = serde_json::from_slice(manifest_bytes)
            .map_err(|_| invalid("bundle manifest is invalid"))?;
        if serde_json::to_vec(&manifest).map_err(|_| invalid("bundle manifest is invalid"))?
            != manifest_bytes
        {
            return Err(invalid("bundle manifest is not canonical"));
        }
        if manifest.schema_version != WORKER_BUNDLE_SCHEMA_VERSION
            || manifest.modules.is_empty()
            || manifest.modules.len() > limits.max_modules
        {
            return Err(invalid("bundle schema or module count is invalid"));
        }
        let main = canonical_module_name(&manifest.main_module)?;
        if main != manifest.main_module {
            return Err(invalid("main module name is not canonical"));
        }
        let mut expected_offset = 0_u64;
        let mut prior: Option<&str> = None;
        let mut saw_main = false;
        for module in &manifest.modules {
            if canonical_module_name(&module.name)? != module.name {
                return Err(invalid("module name is not canonical"));
            }
            if prior.is_some_and(|name| name.as_bytes() >= module.name.as_bytes()) {
                return Err(invalid("module order or uniqueness is invalid"));
            }
            prior = Some(&module.name);
            if module.offset != expected_offset {
                return Err(invalid("module offsets are not contiguous"));
            }
            let size = usize::try_from(module.size).map_err(|_| too_large())?;
            if size > limits.max_module_bytes {
                return Err(too_large());
            }
            let start = blob_offset
                .checked_add(usize::try_from(module.offset).map_err(|_| too_large())?)
                .ok_or_else(too_large)?;
            let end = start.checked_add(size).ok_or_else(too_large)?;
            let raw = bytes
                .get(start..end)
                .ok_or_else(|| invalid("module bytes are truncated"))?;
            if hex::encode(Sha256::digest(raw)) != module.sha256 {
                return Err(PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "module digest does not match the canonical manifest",
                ));
            }
            if matches!(module.module_type, ModuleType::Text | ModuleType::Json)
                && std::str::from_utf8(raw).is_err()
            {
                return Err(invalid("text and JSON modules must be valid UTF-8"));
            }
            if module.module_type == ModuleType::Json
                && serde_json::from_slice::<serde_json::Value>(raw).is_err()
            {
                return Err(invalid("JSON module is invalid"));
            }
            if module.name == manifest.main_module {
                if module.module_type != ModuleType::EsModule {
                    return Err(invalid("main module must be an ES module"));
                }
                saw_main = true;
            }
            expected_offset = expected_offset
                .checked_add(module.size)
                .ok_or_else(too_large)?;
        }
        let expected_end = blob_offset
            .checked_add(usize::try_from(expected_offset).map_err(|_| too_large())?)
            .ok_or_else(too_large)?;
        if !saw_main || expected_end != bytes.len() {
            return Err(invalid(
                "bundle has a missing main module or trailing bytes",
            ));
        }
        if usize::try_from(expected_offset).map_err(|_| too_large())?
            > limits.max_total_module_bytes
        {
            return Err(too_large());
        }
        let sha256 = Sha256::digest(&bytes).into();
        Ok(Self {
            bytes,
            manifest,
            sha256,
            blob_offset,
        })
    }

    /// Canonical artifact bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume into canonical artifact bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Verified manifest.
    #[must_use]
    pub const fn manifest(&self) -> &WorkerBundleManifest {
        &self.manifest
    }

    /// Whole artifact digest.
    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    /// Raw bytes for a manifest module.
    pub fn module_bytes(&self, module: &ModuleManifest) -> Result<&[u8], PlatformError> {
        let start = self
            .blob_offset
            .checked_add(usize::try_from(module.offset).map_err(|_| too_large())?)
            .ok_or_else(too_large)?;
        let end = start
            .checked_add(usize::try_from(module.size).map_err(|_| too_large())?)
            .ok_or_else(too_large)?;
        self.bytes
            .get(start..end)
            .ok_or_else(|| invalid("module range is outside the artifact"))
    }
}

fn parse_manifest(
    manifest_bytes: &[u8],
    limits: BundleLimits,
) -> Result<WorkerBundleManifest, PlatformError> {
    let manifest: WorkerBundleManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|_| invalid("bundle manifest is invalid"))?;
    if serde_json::to_vec(&manifest).map_err(|_| invalid("bundle manifest is invalid"))?
        != manifest_bytes
    {
        return Err(invalid("bundle manifest is not canonical"));
    }
    if manifest.schema_version != WORKER_BUNDLE_SCHEMA_VERSION
        || manifest.modules.is_empty()
        || manifest.modules.len() > limits.max_modules
    {
        return Err(invalid("bundle schema or module count is invalid"));
    }
    if canonical_module_name(&manifest.main_module)? != manifest.main_module {
        return Err(invalid("main module name is not canonical"));
    }
    let mut expected_offset = 0_u64;
    let mut prior: Option<&str> = None;
    let mut saw_main = false;
    for module in &manifest.modules {
        if canonical_module_name(&module.name)? != module.name {
            return Err(invalid("module name is not canonical"));
        }
        if prior.is_some_and(|name| name.as_bytes() >= module.name.as_bytes()) {
            return Err(invalid("module order or uniqueness is invalid"));
        }
        prior = Some(&module.name);
        if module.offset != expected_offset {
            return Err(invalid("module offsets are not contiguous"));
        }
        let module_size = usize::try_from(module.size).map_err(|_| too_large())?;
        if module_size > limits.max_module_bytes {
            return Err(too_large());
        }
        if module.name == manifest.main_module {
            if module.module_type != ModuleType::EsModule {
                return Err(invalid("main module must be an ES module"));
            }
            saw_main = true;
        }
        expected_offset = expected_offset
            .checked_add(module.size)
            .ok_or_else(too_large)?;
    }
    if !saw_main {
        return Err(invalid("bundle main module was not found"));
    }
    if usize::try_from(expected_offset).map_err(|_| too_large())? > limits.max_total_module_bytes {
        return Err(too_large());
    }
    Ok(manifest)
}

fn read_exact_bundle(
    reader: &mut impl Read,
    bytes: &mut [u8],
    message: &'static str,
) -> Result<(), PlatformError> {
    reader.read_exact(bytes).map_err(|_| invalid(message))
}

fn canonical_module_name(name: &str) -> Result<String, PlatformError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.ends_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return Err(invalid("module name is not canonical"));
    }
    let normalized: String = name.nfc().collect();
    if normalized.len() > 512
        || normalized
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(invalid("module name is not canonical"));
    }
    Ok(normalized)
}

fn too_large_or_invalid(empty: bool) -> PlatformError {
    if empty {
        invalid("bundle must contain at least one module")
    } else {
        too_large()
    }
}

fn too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleTooLarge,
        "bundle exceeds a configured structural limit",
    )
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::BundleInvalid, message)
}
