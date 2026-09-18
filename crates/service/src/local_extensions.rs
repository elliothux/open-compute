//! Static local-extension manifest and file loading.

use open_compute_core::{ErrorCode, LocalExtensionConfig, PlatformError};
use open_compute_runtime::{VerifiedLaunchImage, open_host_directory_nofollow};
use rustix::fd::OwnedFd;
use rustix::fs::{Mode, OFlags};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};
use std::sync::Arc;

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_FACADE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    worker: Worker,
    native: Native,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Worker {
    main: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Native {
    executable: std::path::PathBuf,
}

/// One extension opened from the exact files validated at startup.
#[derive(Debug)]
pub(crate) struct LoadedLocalExtension {
    pub(crate) main_module: String,
    pub(crate) facade: Arc<[u8]>,
    pub(crate) executable: Arc<VerifiedLaunchImage>,
    pub(crate) executable_sha256: String,
}

/// Current startup's securely opened local extensions.
#[derive(Debug)]
pub(crate) struct LocalExtensionRegistry {
    entries: BTreeMap<String, LoadedLocalExtension>,
}

impl LocalExtensionRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub(crate) fn load(
        configs: &BTreeMap<String, LocalExtensionConfig>,
    ) -> Result<Self, PlatformError> {
        let entries = configs
            .iter()
            .map(|(name, config)| load_one(config).map(|extension| (name.clone(), extension)))
            .collect::<Result<_, _>>()?;
        Ok(Self { entries })
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&LoadedLocalExtension> {
        self.entries.get(name)
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

fn load_one(config: &LocalExtensionConfig) -> Result<LoadedLocalExtension, PlatformError> {
    let root = open_host_directory_nofollow(&config.path).map_err(|_| invalid())?;
    let manifest = read_bounded(
        open_file(&root, Path::new("extension.toml"))?,
        MAX_MANIFEST_BYTES,
    )?;
    let manifest = std::str::from_utf8(&manifest).map_err(|_| invalid())?;
    let manifest: Manifest = toml::from_str(manifest).map_err(|_| invalid())?;
    let facade = read_bounded(open_file(&root, &manifest.worker.main)?, MAX_FACADE_BYTES)?;
    std::str::from_utf8(&facade).map_err(|_| invalid())?;
    let executable = open_file(&root, &manifest.native.executable)?;
    let metadata = executable.metadata().map_err(|_| invalid())?;
    let mode = metadata.permissions().mode();
    if !metadata.is_file()
        || metadata.len() > MAX_EXECUTABLE_BYTES
        || mode & 0o100 == 0
        || mode & 0o022 != 0
    {
        return Err(invalid());
    }
    let executable_sha256 = sha256_file(&executable)?;
    Ok(LoadedLocalExtension {
        main_module: manifest.worker.main.to_string_lossy().into_owned(),
        facade: facade.into(),
        executable: Arc::new(VerifiedLaunchImage::from_verified_file(executable)),
        executable_sha256,
    })
}

fn sha256_file(file: &File) -> Result<String, PlatformError> {
    let mut file = file.try_clone().map_err(|_| invalid())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let amount = file.read(&mut buffer).map_err(|_| invalid())?;
        if amount == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..amount]);
    }
}

fn open_file(root: &OwnedFd, relative: &Path) -> Result<File, PlatformError> {
    let mut components = relative.components().peekable();
    let mut parent = root.try_clone().map_err(|_| invalid())?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(invalid());
        };
        if components.peek().is_some() {
            parent = rustix::fs::openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| invalid())?;
        } else {
            let fd = rustix::fs::openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|_| invalid())?;
            let file = File::from(fd);
            if !file.metadata().map_err(|_| invalid())?.is_file() {
                return Err(invalid());
            }
            return Ok(file);
        }
    }
    Err(invalid())
}

fn read_bounded(file: File, limit: u64) -> Result<Vec<u8>, PlatformError> {
    if file.metadata().map_err(|_| invalid())?.len() > limit {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(bytes)
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "local extension manifest or files are invalid",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extension_load_is_strict_bounded_and_nofollow() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("extension");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("extension.toml"),
            "[worker]\nmain = 'facade.js'\n[native]\nexecutable = 'provider'\n",
        )
        .unwrap();
        fs::write(root.join("facade.js"), "export default {};").unwrap();
        fs::write(root.join("provider"), "provider").unwrap();
        let mut permissions = fs::metadata(root.join("provider")).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(root.join("provider"), permissions).unwrap();
        let config = LocalExtensionConfig { path: root.clone() };
        let loaded = load_one(&config).unwrap();
        assert_eq!(&*loaded.facade, b"export default {};");
        assert_eq!(loaded.executable_sha256.len(), 64);

        fs::write(root.join("facade.js"), [0xff]).unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::write(root.join("facade.js"), "export default {};").unwrap();

        fs::OpenOptions::new()
            .write(true)
            .open(root.join("provider"))
            .unwrap()
            .set_len(MAX_EXECUTABLE_BYTES + 1)
            .unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::write(root.join("provider"), "provider").unwrap();

        let mut permissions = fs::metadata(root.join("provider")).unwrap().permissions();
        permissions.set_mode(0o722);
        fs::set_permissions(root.join("provider"), permissions).unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        let mut permissions = fs::metadata(root.join("provider")).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(root.join("provider"), permissions).unwrap();

        fs::write(
            root.join("extension.toml"),
            "version = 1\n[worker]\nmain = 'facade.js'\n[native]\nexecutable = 'provider'\n",
        )
        .unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );

        fs::write(
            root.join("extension.toml"),
            "[worker]\nmain = '../facade.js'\n[native]\nexecutable = 'provider'\n",
        )
        .unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );

        fs::write(
            root.join("extension.toml"),
            "[worker]\nmain = 'facade.js'\n[native]\nexecutable = 'provider'\n",
        )
        .unwrap();
        fs::remove_file(root.join("facade.js")).unwrap();
        std::os::unix::fs::symlink(temporary.path().join("outside.js"), root.join("facade.js"))
            .unwrap();
        fs::write(temporary.path().join("outside.js"), "outside").unwrap();
        assert_eq!(
            load_one(&config).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
}
