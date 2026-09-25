//! Static local-extension manifest and file loading.

use open_compute_core::{
    ErrorCode, InstanceId, LocalExtensionConfig, PlatformError, PrivateHttpServiceConfig,
    VersionId, WorkerId,
};
use open_compute_runtime::{VerifiedLaunchImage, open_host_directory_nofollow};
use rustix::fd::OwnedFd;
use rustix::fs::{Mode, OFlags};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
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
    pub(crate) policy_revision: String,
}

/// Current startup's securely opened local extensions.
#[derive(Debug)]
pub(crate) struct LocalExtensionRegistry {
    entries: BTreeMap<String, LoadedLocalExtension>,
    private_services: BTreeMap<String, PrivateHttpTarget>,
}

#[derive(Clone)]
pub(crate) struct PrivateHttpTarget {
    pub(crate) base_url: String,
    pub(crate) methods: std::collections::BTreeSet<String>,
    pub(crate) path_prefixes: Vec<String>,
    pub(crate) credential: Option<(reqwest::header::HeaderName, reqwest::header::HeaderValue)>,
    pub(crate) policy_revision: String,
    pub(crate) grants: Vec<open_compute_core::PrivateHttpGrant>,
    pub(crate) client: reqwest::Client,
}

impl std::fmt::Debug for PrivateHttpTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivateHttpTarget")
            .field("policy_revision", &self.policy_revision)
            .finish_non_exhaustive()
    }
}

impl LocalExtensionRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
            private_services: BTreeMap::new(),
        }
    }

    pub(crate) fn load(
        configs: &BTreeMap<String, LocalExtensionConfig>,
        private_services: &BTreeMap<String, PrivateHttpServiceConfig>,
    ) -> Result<Self, PlatformError> {
        let entries = configs
            .iter()
            .map(|(name, config)| load_one(config).map(|extension| (name.clone(), extension)))
            .collect::<Result<_, _>>()?;
        let private_services = private_services
            .iter()
            .map(|(name, config)| load_private_http(config).map(|target| (name.clone(), target)))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            entries,
            private_services,
        })
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name) || self.private_services.contains_key(name)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&LoadedLocalExtension> {
        self.entries.get(name)
    }

    pub(crate) fn service_target(
        &self,
        name: &str,
        account_id: InstanceId,
        worker_id: WorkerId,
        version_id: Option<VersionId>,
        entrypoint: Option<&str>,
    ) -> Option<open_compute_storage::ServiceTarget> {
        if let Some(extension) = self.entries.get(name) {
            return Some(open_compute_storage::ServiceTarget::Extension {
                name: name.to_owned(),
                policy_revision: extension.policy_revision.clone(),
            });
        }
        self.private_http(name, account_id, worker_id, version_id, entrypoint)
            .map(|target| open_compute_storage::ServiceTarget::Extension {
                name: name.to_owned(),
                policy_revision: target.policy_revision.clone(),
            })
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .keys()
            .chain(self.private_services.keys())
            .map(String::as_str)
    }

    pub(crate) fn native_names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub(crate) fn private_http(
        &self,
        name: &str,
        account_id: InstanceId,
        worker_id: WorkerId,
        version_id: Option<VersionId>,
        entrypoint: Option<&str>,
    ) -> Option<&PrivateHttpTarget> {
        let target = self.private_services.get(name)?;
        target
            .grants
            .iter()
            .any(|grant| {
                grant.account_id == account_id
                    && grant.worker_id == worker_id
                    && match (version_id, grant.version_id) {
                        (Some(actual), Some(expected)) => actual == expected,
                        (_, None) | (None, Some(_)) => true,
                    }
                    && grant.entrypoint.as_deref() == entrypoint
            })
            .then_some(target)
    }

    pub(crate) fn private_http_by_name(&self, name: &str) -> Option<&PrivateHttpTarget> {
        self.private_services.get(name)
    }
}

fn load_private_http(
    config: &PrivateHttpServiceConfig,
) -> Result<PrivateHttpTarget, PlatformError> {
    let address = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|_| invalid_private_http())?
        .find(|address| private_address(address.ip()))
        .ok_or_else(invalid_private_http)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .resolve(&config.host, address)
        .build()
        .map_err(|_| invalid_private_http())?;
    let credential = match (&config.credential_header, &config.credential) {
        (Some(name), Some(value)) => {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| invalid_private_http())?;
            let secret = crate::auth::resolve_admin_auth(value)?;
            let mut value = reqwest::header::HeaderValue::from_str(secret.expose())
                .map_err(|_| invalid_private_http())?;
            value.set_sensitive(true);
            Some((name, value))
        }
        (None, None) => None,
        _ => return Err(invalid_private_http()),
    };
    let policy_revision = hex::encode(Sha256::digest(
        serde_json::to_vec(config).map_err(|_| invalid_private_http())?,
    ));
    let host = match config.host.parse::<IpAddr>() {
        Ok(IpAddr::V6(value)) => format!("[{value}]"),
        _ => config.host.clone(),
    };
    Ok(PrivateHttpTarget {
        base_url: format!("{}://{}:{}", config.scheme, host, config.port),
        methods: config.methods.clone(),
        path_prefixes: config.path_prefixes.clone(),
        credential,
        policy_revision,
        grants: config.allow.clone(),
        client,
    })
}

fn private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => value.is_private() || value.is_loopback() || value.is_link_local(),
        IpAddr::V6(value) => {
            value.is_loopback() || value.is_unique_local() || value.is_unicast_link_local()
        }
    }
}

fn invalid_private_http() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "private HTTP Service target could not be pinned safely",
    )
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
    let policy_revision = hex::encode(Sha256::digest(
        [
            manifest.worker.main.to_string_lossy().as_bytes(),
            facade.as_slice(),
            executable_sha256.as_bytes(),
        ]
        .concat(),
    ));
    Ok(LoadedLocalExtension {
        main_module: manifest.worker.main.to_string_lossy().into_owned(),
        facade: facade.into(),
        executable: Arc::new(VerifiedLaunchImage::from_verified_file(executable)),
        executable_sha256,
        policy_revision,
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

    #[test]
    fn private_http_target_is_pinned_and_grants_are_rechecked() {
        let account = InstanceId::generate();
        let worker = WorkerId::generate();
        let version = VersionId::generate();
        let config = PrivateHttpServiceConfig {
            scheme: "http".to_owned(),
            host: "::1".to_owned(),
            port: 8080,
            path_prefixes: vec!["/v1/".to_owned()],
            methods: ["GET".to_owned()].into_iter().collect(),
            credential_header: None,
            credential: None,
            allow: vec![open_compute_core::PrivateHttpGrant {
                account_id: account,
                worker_id: worker,
                version_id: Some(version),
                entrypoint: None,
            }],
        };
        let registry = LocalExtensionRegistry::load(
            &BTreeMap::new(),
            &BTreeMap::from([("inventory".to_owned(), config.clone())]),
        )
        .unwrap();
        let target = registry.private_http_by_name("inventory").unwrap();
        assert_eq!(target.base_url, "http://[::1]:8080");
        assert_eq!(target.policy_revision.len(), 64);
        assert!(format!("{target:?}").contains(&target.policy_revision));
        assert!(registry.contains("inventory"));
        assert_eq!(registry.names().collect::<Vec<_>>(), ["inventory"]);
        assert_eq!(registry.native_names().count(), 0);
        assert_eq!(
            registry
                .service_target("inventory", account, worker, Some(version), None)
                .unwrap(),
            open_compute_storage::ServiceTarget::Extension {
                name: "inventory".to_owned(),
                policy_revision: target.policy_revision.clone(),
            }
        );
        assert!(
            registry
                .private_http("inventory", account, worker, None, None)
                .is_some()
        );
        assert!(
            registry
                .private_http("inventory", account, worker, Some(version), None)
                .is_some()
        );
        assert!(
            registry
                .private_http(
                    "inventory",
                    account,
                    worker,
                    Some(VersionId::generate()),
                    None,
                )
                .is_none()
        );
        assert!(
            registry
                .private_http("missing", account, worker, Some(version), None)
                .is_none()
        );
        assert!(!private_address("203.0.113.10".parse().unwrap()));
        assert!(private_address("127.0.0.1".parse().unwrap()));
        assert!(private_address("169.254.1.1".parse().unwrap()));
        assert!(private_address("10.0.0.1".parse().unwrap()));
        assert!(private_address("fc00::1".parse().unwrap()));
        assert!(private_address("fe80::1".parse().unwrap()));

        let mut invalid = config.clone();
        invalid.host = "203.0.113.10".to_owned();
        assert_eq!(
            load_private_http(&invalid).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        invalid = config;
        invalid.credential_header = Some("x-api-key".to_owned());
        assert_eq!(
            load_private_http(&invalid).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        let temporary = tempfile::tempdir().unwrap();
        let secret = temporary.path().join("credential");
        fs::write(&secret, "operator-secret\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        invalid.credential = Some(open_compute_core::SecretReference {
            env: None,
            file: Some(secret),
        });
        let credential = load_private_http(&invalid).unwrap().credential.unwrap();
        assert_eq!(credential.0, "x-api-key");
        assert!(credential.1.is_sensitive());
    }
}
