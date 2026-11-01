//! Static workerd config compiler and on-disk cache.

use crate::digest::digest_for_with_policy;
use crate::fsutil::{
    FILE_MODE, MAX_LOCK_BYTES, WorkDir, chmod, contained_in, create_dir_secure, fsync_dir,
    hex_sha256, open_dir, open_file, read_regular_file_bounded, remove_file_nofollow,
    remove_file_strict, rename_noreplace, require_absolute, write_atomic_new,
};
use crate::process::assert_reaped;
use crate::verify::VerifiedRuntime;
use open_compute_core::{DurableObjectsConfig, ErrorCode, PlatformError, Redactor};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const MAX_COMPILED_BYTES: usize = 16 * 1024 * 1024;

pub use crate::digest::PlatformReleaseMeta;

pub(crate) fn cleanup_interrupted_compile_state(runtime_dir: &Path) -> Result<(), PlatformError> {
    let directory = open_dir(runtime_dir)?;
    let entries = rustix::fs::Dir::read_from(&directory).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to inspect runtime compile staging",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "failed to inspect runtime compile staging",
            )
        })?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        let path = runtime_dir.join(name);
        if is_compile_workdir(name) {
            let _ = open_dir(&path)?;
            fs::remove_dir_all(&path).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "failed to recover interrupted runtime compilation",
                )
            })?;
        } else if is_atomic_partial(name) {
            remove_file_nofollow(&path)?;
        }
    }
    fsync_dir(runtime_dir)
}

fn is_compile_workdir(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(".compile.")
        .or_else(|| name.strip_prefix(".partial."))
    else {
        return false;
    };
    let Some((digest, id)) = rest.rsplit_once('.') else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && uuid::Uuid::parse_str(id).is_ok_and(|id| id.get_version_num() == 7)
}

fn is_atomic_partial(name: &str) -> bool {
    name.strip_prefix(".partial.")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .is_some_and(|id| id.get_version_num() == 7)
}

/// Checked-in process profile selected before workerd compiles its configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigRole {
    /// The instance runtime, including its sole Durable Object storage authority.
    Runtime,
    /// Disposable Python preparation, delegating DO operations to the instance runtime.
    PythonPreparation,
}

impl ConfigRole {
    pub(crate) const fn constant(self) -> &'static str {
        match self {
            Self::Runtime => "config",
            Self::PythonPreparation => "prepareConfig",
        }
    }
}

/// Inputs required to compile or reuse a binary config.
pub struct CompileRequest<'a> {
    /// Exact static profile to compile; also part of the cache identity.
    pub role: ConfigRole,
    /// Verified workerd identity and opened executable.
    pub runtime: &'a VerifiedRuntime,
    /// Absolute lock file used for digest mixing. Must match the verified lock bytes.
    pub lock_path: &'a Path,
    /// Absolute packaged runtime assets directory.
    pub assets_dir: &'a Path,
    /// Absolute `<data>/runtime` directory.
    pub runtime_data_dir: &'a Path,
    /// Platform release metadata mixed into the digest.
    pub platform: &'a PlatformReleaseMeta,
    /// Absolute private Unix socket served by this config.
    pub ingress_socket: &'a Path,
    /// Validated Durable Object policy rendered into private system-Worker bindings.
    pub durable_objects: DurableObjectsConfig,
    /// Qualified native Container provider, scoped to the runtime `DoHost` Worker.
    pub container_provider: Option<&'a crate::ContainerProviderConfig>,
    /// Compile deadline.
    pub deadline: Duration,
    /// Redactor for compiler diagnostics.
    pub redactor: &'a Redactor,
}

impl Debug for CompileRequest<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompileRequest")
            .field("role", &self.role)
            .field("runtime", &self.runtime)
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

/// Handle to a verified compiled config. Debug/Display omit filesystem paths and secrets.
pub struct CompiledConfig {
    role: ConfigRole,
    digest: String,
    path: PathBuf,
    content_sha256: String,
    ingress_socket: PathBuf,
}

impl Debug for CompiledConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledConfig")
            .field("digest", &self.digest)
            .finish()
    }
}

impl Display for CompiledConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "CompiledConfig(digest={})", self.digest)
    }
}

impl CompiledConfig {
    /// Static process profile bound to this verified cache handle.
    #[must_use]
    pub fn role(&self) -> ConfigRole {
        self.role
    }

    /// Input digest identifying this compiled config.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Absolute path for the supervisor. Not shown in Debug/Display.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Private ingress endpoint rendered into this config.
    pub fn ingress_socket(&self) -> &Path {
        &self.ingress_socket
    }

    /// Open the compiled config and check its content digest.
    pub fn open(&self) -> Result<File, PlatformError> {
        let (file, _bytes, content) = open_and_hash_compiled(&self.path)?;
        if content != self.content_sha256 {
            return Err(PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "compiled config content digest does not match",
            ));
        }
        validate_sidecar_for(&self.path, &self.digest, &content)?;
        Ok(file)
    }

    /// Read the verified compiled bytes for stdin delivery.
    pub fn read_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        let mut file = self.open()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::CacheEntryCorrupt,
                "failed to read compiled config",
            )
        })?;
        Ok(bytes)
    }

    /// Build a preparation-profile config from explicit fixture bytes.
    #[cfg(any(test, feature = "test-support"))]
    pub fn preparation_from_bytes_for_test(
        dir: &Path,
        digest: &str,
        bytes: &[u8],
    ) -> Result<Self, PlatformError> {
        let mut compiled = Self::from_bytes_for_test(dir, digest, bytes)?;
        compiled.role = ConfigRole::PythonPreparation;
        Ok(compiled)
    }

    /// Build a compiled-config handle from bytes for supervisor tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_bytes_for_test(
        dir: &Path,
        digest: &str,
        bytes: &[u8],
    ) -> Result<Self, PlatformError> {
        create_dir_secure(dir)?;
        let path = dir.join(format!("config.{digest}.bin"));
        write_atomic_new(&path, bytes, FILE_MODE)?;
        write_sidecar(&path, digest, bytes)?;
        let content_sha256 = hex_sha256(&Sha256::digest(bytes).into());
        Ok(Self {
            role: ConfigRole::Runtime,
            digest: digest.to_owned(),
            path,
            content_sha256,
            ingress_socket: dir.join("ingress.sock"),
        })
    }
}

/// Compile the static config or reuse a valid cache entry.
pub async fn compile_static_config(
    request: CompileRequest<'_>,
) -> Result<CompiledConfig, PlatformError> {
    require_absolute(request.assets_dir)?;
    require_absolute(request.runtime_data_dir)?;
    let _ = open_dir(request.assets_dir)?;
    require_absolute(request.lock_path)?;
    let lock_bytes = read_regular_file_bounded(request.lock_path, MAX_LOCK_BYTES)?;
    if lock_bytes != request.runtime.lock_bytes() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "lock path contents do not match the verified lock identity",
        ));
    }
    let (digest, rendered, workers) = digest_for_with_policy(
        request.assets_dir,
        &lock_bytes,
        request.runtime,
        request.platform,
        request.ingress_socket,
        &request.durable_objects,
        request.container_provider,
        request.role,
    )?;

    create_dir_secure(request.runtime_data_dir)?;
    let dest = request
        .runtime_data_dir
        .join(format!("config.{digest}.bin"));
    contained_in(request.runtime_data_dir, &dest)?;

    {
        let publish_gate = digest_publish_mutex(&digest);
        let _publish = publish_gate.lock().await;
        if let Some(content_sha256) = try_reuse_or_clear_cache(&dest, &digest)? {
            return Ok(CompiledConfig {
                role: request.role,
                digest,
                path: dest,
                content_sha256,
                ingress_socket: request.ingress_socket.to_owned(),
            });
        }
    }

    let work_dir = WorkDir::create(request.runtime_data_dir, &format!(".compile.{digest}"))?;
    compile_into(
        work_dir.path(),
        &dest,
        &digest,
        &rendered,
        &workers,
        &request,
    )
    .await
}

async fn compile_into(
    work_dir: &Path,
    dest: &Path,
    digest: &str,
    rendered: &str,
    workers: &[(String, Vec<u8>)],
    request: &CompileRequest<'_>,
) -> Result<CompiledConfig, PlatformError> {
    let generated = work_dir.join("config.capnp");
    write_atomic_new(&generated, rendered.as_bytes(), FILE_MODE)?;
    for (rel, bytes) in workers {
        let path = work_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "failed to create worker copy directory",
                )
            })?;
        }
        write_atomic_new(&path, bytes, FILE_MODE)?;
    }

    let partial_guard = WorkDir::create(request.runtime_data_dir, &format!(".partial.{digest}"))?;
    let partial = partial_guard.path().join("config.bin");
    let stdout_file = open_file(&partial, true, true)?;
    chmod(&partial, FILE_MODE)?;

    let generated_str = generated.to_str().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "generated config path is not UTF-8",
        )
    })?;
    let output = request
        .runtime
        .run(
            &[
                "compile",
                generated_str,
                request.role.constant(),
                "--config-only",
            ],
            request.deadline,
            MAX_COMPILED_BYTES,
            request.redactor,
            Some(stdout_file),
        )
        .await;
    let output = match output {
        Ok(o) => o,
        Err(err) => return Err(err),
    };
    let reap = assert_reaped(output.pid);
    if output.timed_out || output.stdout_overflow || !output.status.is_some_and(|s| s.success()) {
        reap?;
        if output.timed_out {
            return Err(PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "workerd compile timed out",
            ));
        }
        if output.stdout_overflow {
            return Err(PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "workerd compile output exceeded the bound",
            ));
        }
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "workerd compile exited unsuccessfully",
        ));
    }
    reap?;

    let (compiled, bytes, content_hash) = open_and_hash_compiled(&partial)?;
    if bytes.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "workerd compile produced an empty config",
        ));
    }
    compiled.sync_all().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to fsync compiled config",
        )
    })?;
    drop(compiled);

    let publish_gate = digest_publish_mutex(digest);
    let _publish = publish_gate.lock().await;
    if let Some(existing) = try_reuse_or_clear_cache(dest, digest)? {
        return Ok(CompiledConfig {
            role: request.role,
            digest: digest.to_owned(),
            path: dest.to_path_buf(),
            content_sha256: existing,
            ingress_socket: request.ingress_socket.to_owned(),
        });
    }

    match rename_noreplace(&partial, dest) {
        Ok(()) => {
            chmod(dest, FILE_MODE)?;
            fsync_dir(request.runtime_data_dir)?;
            run_after_config_rename_hook(dest);
            write_sidecar(dest, digest, &bytes)?;
            Ok(CompiledConfig {
                role: request.role,
                digest: digest.to_owned(),
                path: dest.to_path_buf(),
                content_sha256: content_hash,
                ingress_socket: request.ingress_socket.to_owned(),
            })
        }
        Err(_) => match wait_reuse_winner(dest, digest).await {
            Ok(existing) => Ok(CompiledConfig {
                role: request.role,
                digest: digest.to_owned(),
                path: dest.to_path_buf(),
                content_sha256: existing,
                ingress_socket: request.ingress_socket.to_owned(),
            }),
            Err(err) => Err(err),
        },
    }
}

fn digest_publish_mutex(digest: &str) -> Arc<tokio::sync::Mutex<()>> {
    static GATES: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let gates = GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = gates
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(digest.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn try_reuse_or_clear_cache(dest: &Path, digest: &str) -> Result<Option<String>, PlatformError> {
    if !dest.exists() {
        return Ok(None);
    }
    match validate_cache_entry(dest, digest) {
        Ok(existing) => Ok(Some(existing)),
        Err(err) => {
            remove_file_strict(dest)?;
            remove_file_strict(&sidecar_path(dest))?;
            if dest.exists() || sidecar_path(dest).exists() {
                return Err(err);
            }
            Ok(None)
        }
    }
}

async fn wait_reuse_winner(dest: &Path, digest: &str) -> Result<String, PlatformError> {
    let mut last = PlatformError::new(
        ErrorCode::CacheEntryCorrupt,
        "compiled config lost the no-replace publish race",
    );
    for _ in 0..100 {
        match validate_cache_entry(dest, digest) {
            Ok(content) => return Ok(content),
            Err(err) => last = err,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(last)
}

fn sidecar_path(dest: &Path) -> PathBuf {
    dest.with_extension("bin.digest")
}

fn write_sidecar(dest: &Path, digest: &str, bytes: &[u8]) -> Result<(), PlatformError> {
    let content_hash = hex_sha256(&Sha256::digest(bytes).into());
    let body = format!("{digest}\n{content_hash}\n");
    write_atomic_new(&sidecar_path(dest), body.as_bytes(), FILE_MODE)
}

fn validate_cache_entry(path: &Path, expected_digest: &str) -> Result<String, PlatformError> {
    let (_file, _bytes, content) = open_and_hash_compiled(path)?;
    validate_sidecar_for(path, expected_digest, &content)?;
    Ok(content)
}

fn open_and_hash_compiled(path: &Path) -> Result<(File, Vec<u8>, String), PlatformError> {
    let mut file = open_file(path, false, false)?;
    let meta = file.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to stat compiled config",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config must be a regular file",
        ));
    }
    if meta.len() == 0 || meta.len() as usize > MAX_COMPILED_BYTES {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config size is invalid",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to read compiled config",
        )
    })?;
    if bytes.is_empty() || bytes.len() as u64 != meta.len() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config is empty",
        ));
    }
    let content = hex_sha256(&Sha256::digest(&bytes).into());
    use std::io::Seek;
    file.rewind().map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to rewind compiled config",
        )
    })?;
    Ok((file, bytes, content))
}

fn validate_sidecar_for(
    path: &Path,
    expected_digest: &str,
    content: &str,
) -> Result<(), PlatformError> {
    let sidecar_path = sidecar_path(path);
    let mut sidecar = open_file(&sidecar_path, false, false)?;
    let meta = sidecar.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to stat compiled config digest sidecar",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config digest sidecar must be a regular file",
        ));
    }
    if meta.len() == 0 || meta.len() as usize > 256 {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config digest sidecar size is invalid",
        ));
    }
    let mut text_bytes = Vec::new();
    sidecar.read_to_end(&mut text_bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "failed to read compiled config digest sidecar",
        )
    })?;
    let text = std::str::from_utf8(&text_bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config digest sidecar is invalid",
        )
    })?;
    let mut lines = text.lines();
    let digest = lines.next().unwrap_or("");
    let stored = lines.next().unwrap_or("");
    if digest != expected_digest {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config digest sidecar does not match",
        ));
    }
    if stored != content {
        return Err(PlatformError::new(
            ErrorCode::CacheEntryCorrupt,
            "compiled config content digest does not match",
        ));
    }
    Ok(())
}

#[cfg(test)]
type AfterRenameHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
static AFTER_RENAME_HOOK: Mutex<Option<AfterRenameHook>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_after_config_rename_hook(hook: impl Fn(&Path) + Send + Sync + 'static) {
    *AFTER_RENAME_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_after_config_rename_hook() {
    *AFTER_RENAME_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn run_after_config_rename_hook(dest: &Path) {
    #[cfg(test)]
    {
        if let Some(hook) = AFTER_RENAME_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook(dest);
        }
    }
    #[cfg(not(test))]
    {
        let _ = dest;
    }
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod coverage_tests;
