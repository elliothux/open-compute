//! Secure local bare-repository ownership for Cloudflare Artifacts.

use open_compute_core::{ArtifactRepoId, ErrorCode, PlatformError};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

mod import_http;
use import_http::{PinnedHttp, remote_invalid, upstream_unavailable, validate_public_remote};

/// Direct bare-Git repository authority. Paths are derived only from typed IDs.
#[derive(Clone, Debug)]
pub struct GitRepositoryStore {
    root: PathBuf,
    quarantine: PathBuf,
    max_object_bytes: usize,
}

/// One immutable Git object returned by the REST content surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitObject {
    /// Lowercase SHA-1 object identity.
    pub oid: String,
    /// Git object kind.
    pub kind: GitObjectKind,
    /// Exact canonical object payload.
    pub bytes: Vec<u8>,
}

/// Supported Git object type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GitObjectKind {
    /// Commit object.
    Commit,
    /// Tree object.
    Tree,
    /// Blob object.
    Blob,
    /// Annotated tag object.
    Tag,
}

impl GitRepositoryStore {
    /// Validate and canonicalize an import URL before it enters durable metadata.
    pub fn validate_import_url(remote: &str) -> Result<String, PlatformError> {
        import_http::canonical_public_https_remote(remote)
    }

    /// Open an existing secure repository root.
    pub fn open(
        root: PathBuf,
        quarantine: PathBuf,
        max_object_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if !root.is_absolute() || !quarantine.is_absolute() || root == quarantine {
            return Err(unavailable());
        }
        let max_object_bytes = usize::try_from(max_object_bytes).map_err(|_| unavailable())?;
        if max_object_bytes == 0 {
            return Err(unavailable());
        }
        validate_directory(&root)?;
        validate_directory(&quarantine)?;
        Ok(Self {
            root,
            quarantine,
            max_object_bytes,
        })
    }

    /// Absolute path derived from a typed repository identity.
    #[must_use]
    pub fn path(&self, id: ArtifactRepoId) -> PathBuf {
        self.root.join(format!("{id}.git"))
    }

    /// Create and fsync a minimal standards-compliant bare repository without invoking Git.
    pub fn initialize(
        &self,
        id: ArtifactRepoId,
        default_branch: &str,
    ) -> Result<PathBuf, PlatformError> {
        validate_branch(default_branch)?;
        let path = self.path(id);
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(PlatformError::new(
                ErrorCode::ResourceNameConflict,
                "Artifact repository directory already exists",
            ));
        }
        create_private_dir(&path)?;
        let result = (|| {
            for relative in [
                "branches",
                "hooks",
                "info",
                "objects",
                "objects/info",
                "objects/pack",
                "refs",
                "refs/heads",
                "refs/tags",
            ] {
                create_private_dir(&path.join(relative))?;
            }
            write_private_file(
                &path.join("HEAD"),
                format!("ref: refs/heads/{default_branch}\n").as_bytes(),
            )?;
            write_private_file(
                &path.join("config"),
                b"[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = true\n\tlogallrefupdates = true\n",
            )?;
            write_private_file(
                &path.join("description"),
                b"Open Compute Artifact repository\n",
            )?;
            gix::open(&path).map_err(|_| unavailable())?;
            sync_directory(&path)?;
            sync_directory(&self.root)
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&path);
            return Err(error);
        }
        Ok(path)
    }

    /// Verify that a catalog row resolves to one owned bare repository.
    pub fn verify(&self, id: ArtifactRepoId) -> Result<PathBuf, PlatformError> {
        let path = self.path(id);
        validate_directory(&path)?;
        validate_repository_tree(&path)?;
        let repository = gix::open(&path).map_err(|_| unavailable())?;
        if !repository.is_bare() || path.join("objects/info/alternates").exists() {
            return Err(unavailable());
        }
        Ok(path)
    }

    /// Copy one verified repository into a newly reserved identity.
    pub fn fork(
        &self,
        source: ArtifactRepoId,
        destination_id: ArtifactRepoId,
        default_branch_only: bool,
        default_branch: &str,
        max_bytes: u64,
    ) -> Result<PathBuf, PlatformError> {
        let source = self.verify(source)?;
        let destination = self.path(destination_id);
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(unavailable());
        }
        let result = (|| {
            copy_tree(&source, &destination)?;
            if default_branch_only {
                retain_default_branch(&destination, default_branch)?;
            }
            directory_size(&destination, max_bytes)?;
            sync_directory(&destination)?;
            sync_directory(&self.root)?;
            self.verify(destination_id)
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&destination);
        }
        result
    }

    /// Clone one public HTTPS remote into a newly reserved bare repository.
    pub fn import_public_https(
        &self,
        id: ArtifactRepoId,
        remote: &str,
        branch: Option<&str>,
        depth: Option<u32>,
        max_bytes: u64,
        timeout: Duration,
    ) -> Result<PathBuf, PlatformError> {
        let validated = validate_public_remote(remote)?;
        if let Some(branch) = branch {
            validate_branch(branch)?;
        }
        let shallow = depth
            .map(|value| NonZeroU32::new(value).ok_or_else(invalid))
            .transpose()?;
        let destination = self.path(id);
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(unavailable());
        }
        let result = (|| {
            let transport_url = gix::Url::try_from(remote).map_err(|_| remote_invalid())?;
            let pinned_http = PinnedHttp::new(
                validated.host.clone(),
                &validated.addresses,
                max_bytes,
                timeout,
            )?;
            let transport_http = pinned_http.clone();
            let mut prepare = gix::prepare_clone_bare(remote, &destination)
                .map_err(|_| upstream_unavailable())?
                .with_in_memory_config_overrides([
                    "http.followRedirects=false",
                    "http.proxy=",
                    "credential.helper=",
                ])
                .configure_connection(move |connection| {
                    *connection.transport_mut() = Box::new(
                        gix_transport::client::blocking_io::http::Transport::new_http(
                            transport_http.clone(),
                            transport_url.clone(),
                            gix::protocol::transport::Protocol::V2,
                            false,
                        ),
                    );
                    Ok(())
                });
            if let Some(branch) = branch {
                prepare = prepare.with_ref_name(Some(branch)).map_err(|_| invalid())?;
            }
            if let Some(depth) = shallow {
                prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(depth));
            }
            let interrupt = AtomicBool::new(false);
            let (done, wait) = mpsc::sync_channel(1);
            let fetched = std::thread::scope(|scope| {
                let interrupt_ref = &interrupt;
                scope.spawn(move || {
                    if wait.recv_timeout(timeout).is_err() {
                        interrupt_ref.store(true, Ordering::Relaxed);
                    }
                });
                let fetched = prepare.fetch_only(gix::progress::Discard, &interrupt);
                let _ = done.send(());
                fetched
            });
            let (repository, _) = fetched.map_err(|_| pinned_http.failure_error())?;
            drop(repository);
            harden_repository_tree(&destination)?;
            if directory_size(&destination, max_bytes)? > max_bytes {
                return Err(PlatformError::new(
                    ErrorCode::ResourceLimitExceeded,
                    "Artifact imported repository exceeds the configured byte limit",
                ));
            }
            sync_directory(&destination)?;
            sync_directory(&self.root)?;
            self.verify(id)
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&destination);
        }
        result
    }

    /// Read the repository's validated symbolic default branch.
    pub fn default_branch(&self, id: ArtifactRepoId) -> Result<String, PlatformError> {
        let path = self.verify(id)?;
        let head = std::fs::read_to_string(path.join("HEAD")).map_err(|_| unavailable())?;
        let branch = head
            .strip_prefix("ref: refs/heads/")
            .and_then(|value| value.strip_suffix('\n'))
            .ok_or_else(unavailable)?;
        validate_branch(branch)?;
        Ok(branch.to_owned())
    }

    /// Count every loose or packed object in a verified repository.
    pub fn object_count(&self, id: ArtifactRepoId) -> Result<usize, PlatformError> {
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        repository
            .objects
            .store_ref()
            .iter()
            .map_err(|_| unavailable())?
            .try_fold(0_usize, |count, object| {
                object
                    .map_err(|_| unavailable())
                    .and_then(|_| count.checked_add(1).ok_or_else(unavailable))
            })
    }

    /// Reject upload-pack wants that were not disclosed by a repository ref.
    pub fn validate_advertised_wants(
        &self,
        id: ArtifactRepoId,
        wants: &[String],
    ) -> Result<(), PlatformError> {
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        let references = repository.references().map_err(|_| unavailable())?;
        let references = references.all().map_err(|_| unavailable())?;
        let mut advertised = HashSet::new();
        for reference in references {
            let mut reference = reference.map_err(|_| unavailable())?;
            advertised.insert(
                reference
                    .peel_to_id()
                    .map_err(|_| unavailable())?
                    .to_string(),
            );
        }
        if wants.iter().all(|want| advertised.contains(want)) {
            Ok(())
        } else {
            Err(PlatformError::new(
                ErrorCode::ResourceNotFound,
                "Artifact Git object is not advertised",
            ))
        }
    }

    /// Move one fenced repository out of authority and remove its exact quarantine entry.
    pub fn delete(&self, id: ArtifactRepoId) -> Result<(), PlatformError> {
        let path = self.path(id);
        let deleting = self.quarantine.join(format!("{id}.git"));
        if std::fs::symlink_metadata(&path).is_ok() {
            self.verify(id)?;
            if std::fs::symlink_metadata(&deleting).is_ok() {
                return Err(unavailable());
            }
            std::fs::rename(&path, &deleting).map_err(|_| unavailable())?;
            sync_directory(&self.root)?;
        } else if std::fs::symlink_metadata(&deleting).is_err() {
            return Ok(());
        }
        let metadata = std::fs::symlink_metadata(&deleting).map_err(|_| unavailable())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(unavailable());
        }
        std::fs::remove_dir_all(&deleting).map_err(|_| unavailable())?;
        sync_directory(&self.quarantine)
    }

    /// Isolate corrupt or incomplete repository bytes without following them.
    pub fn quarantine_corrupt(&self, id: ArtifactRepoId) -> Result<(), PlatformError> {
        let path = self.path(id);
        let quarantined = self.quarantine.join(format!("{id}.git"));
        match std::fs::symlink_metadata(&path) {
            Ok(_) if std::fs::symlink_metadata(&quarantined).is_err() => {
                std::fs::rename(&path, &quarantined).map_err(|_| unavailable())?;
                sync_directory(&self.root)?;
                sync_directory(&self.quarantine)
            }
            Ok(_) => Err(unavailable()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }

    /// Purge repository quarantine retained from a previous daemon generation.
    pub fn cleanup_quarantine(&self) -> Result<(), PlatformError> {
        for entry in std::fs::read_dir(&self.quarantine).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            let name = entry.file_name().into_string().map_err(|_| unavailable())?;
            let id = name.strip_suffix(".git").ok_or_else(unavailable)?;
            id.parse::<ArtifactRepoId>().map_err(|_| unavailable())?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|_| unavailable())?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                std::fs::remove_dir_all(entry.path()).map_err(|_| unavailable())?;
            } else {
                std::fs::remove_file(entry.path()).map_err(|_| unavailable())?;
            }
        }
        sync_directory(&self.quarantine)
    }

    /// Remove a not-yet-published repository directory during failed-state convergence.
    pub fn discard_unpublished(&self, id: ArtifactRepoId) -> Result<(), PlatformError> {
        let path = self.path(id);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                std::fs::remove_dir_all(path).map_err(|_| unavailable())?;
                sync_directory(&self.root)
            }
            Ok(_) => Err(unavailable()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }

    /// Read an exact object by full SHA-1 without rev-expression evaluation.
    pub fn read_object(&self, id: ArtifactRepoId, oid: &str) -> Result<GitObject, PlatformError> {
        if !valid_oid(oid) {
            return Err(invalid());
        }
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        let object_id = gix::ObjectId::from_hex(oid.as_bytes()).map_err(|_| invalid())?;
        let object = repository.find_object(object_id).map_err(|_| not_found())?;
        if object.data.len() > self.max_object_bytes {
            return Err(PlatformError::new(
                ErrorCode::ResourceLimitExceeded,
                "Artifact Git object exceeds the response limit",
            ));
        }
        let kind = match object.kind {
            gix::objs::Kind::Commit => GitObjectKind::Commit,
            gix::objs::Kind::Tree => GitObjectKind::Tree,
            gix::objs::Kind::Blob => GitObjectKind::Blob,
            gix::objs::Kind::Tag => GitObjectKind::Tag,
        };
        Ok(GitObject {
            oid: oid.to_owned(),
            kind,
            bytes: object.data.to_vec(),
        })
    }

    /// Resolve a full object ID or exact `refs/heads/*` / `refs/tags/*` name.
    pub fn resolve_revision(
        &self,
        id: ArtifactRepoId,
        revision: &str,
    ) -> Result<String, PlatformError> {
        if valid_oid(revision) {
            self.read_object(id, revision)?;
            return Ok(revision.to_owned());
        }
        let revision = if revision.starts_with("refs/heads/") || revision.starts_with("refs/tags/")
        {
            revision.to_owned()
        } else {
            validate_branch(revision)?;
            let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
            let branch = format!("refs/heads/{revision}");
            if repository
                .try_find_reference(&branch)
                .map_err(|_| unavailable())?
                .is_some()
            {
                branch
            } else {
                format!("refs/tags/{revision}")
            }
        };
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        let mut reference = repository
            .find_reference(&revision)
            .map_err(|_| not_found())?;
        let object = reference.peel_to_id().map_err(|_| not_found())?;
        Ok(object.detach().to_string())
    }

    /// Read one regular blob at a validated repository-relative path and revision.
    pub fn read_file(
        &self,
        id: ArtifactRepoId,
        revision: &str,
        relative_path: &str,
    ) -> Result<GitObject, PlatformError> {
        let relative_path = Path::new(relative_path);
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(invalid());
        }
        let revision = self.resolve_revision(id, revision)?;
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        let object = repository
            .find_object(gix::ObjectId::from_hex(revision.as_bytes()).map_err(|_| invalid())?)
            .map_err(|_| not_found())?;
        let commit = object
            .peel_to_kind(gix::object::Kind::Commit)
            .map_err(|_| not_found())?;
        let tree = commit.into_commit().tree().map_err(|_| not_found())?;
        let entry = tree
            .lookup_entry_by_path(relative_path)
            .map_err(|_| not_found())?
            .ok_or_else(not_found)?;
        let object = entry.object().map_err(|_| not_found())?;
        if object.kind != gix::object::Kind::Blob {
            return Err(not_found());
        }
        if object.data.len() > self.max_object_bytes {
            return Err(PlatformError::new(
                ErrorCode::ResourceLimitExceeded,
                "Artifact Git object exceeds the response limit",
            ));
        }
        Ok(GitObject {
            oid: object.id.to_string(),
            kind: GitObjectKind::Blob,
            bytes: object.data.to_vec(),
        })
    }

    /// Read a bounded page of commit objects from one revision.
    pub fn commit_log(
        &self,
        id: ArtifactRepoId,
        revision: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<GitObject>, PlatformError> {
        if limit == 0 || limit > 200 || offset > 100_000 {
            return Err(invalid());
        }
        let revision = self.resolve_revision(id, revision)?;
        let repository = gix::open(self.verify(id)?).map_err(|_| unavailable())?;
        let commit = repository
            .find_object(gix::ObjectId::from_hex(revision.as_bytes()).map_err(|_| invalid())?)
            .map_err(|_| not_found())?
            .peel_to_kind(gix::object::Kind::Commit)
            .map_err(|_| not_found())?
            .into_commit();
        let mut result = Vec::new();
        for item in commit
            .ancestors()
            .all()
            .map_err(|_| unavailable())?
            .skip(offset)
            .take(limit)
        {
            let info = item.map_err(|_| unavailable())?;
            let object = info.object().map_err(|_| unavailable())?;
            result.push(GitObject {
                oid: info.id.to_string(),
                kind: GitObjectKind::Commit,
                bytes: object.data.to_vec(),
            });
        }
        Ok(result)
    }

    /// Verify and measure one repository without traversing outside its owned root.
    pub fn repository_size(&self, id: ArtifactRepoId, limit: u64) -> Result<u64, PlatformError> {
        directory_size(&self.verify(id)?, limit)
    }
}

fn retain_default_branch(path: &Path, default_branch: &str) -> Result<(), PlatformError> {
    use gix::bstr::ByteSlice as _;

    validate_branch(default_branch)?;
    let keep = format!("refs/heads/{default_branch}");
    let repository = gix::open(path).map_err(|_| unavailable())?;
    let references = repository.references().map_err(|_| unavailable())?;
    let mut remove = Vec::new();
    for reference in references.all().map_err(|_| unavailable())? {
        let reference = reference.map_err(|_| unavailable())?;
        let name = reference
            .name()
            .as_bstr()
            .to_str()
            .map_err(|_| unavailable())?;
        if name != keep {
            remove.push(name.to_owned());
        }
    }
    for name in remove {
        let reference = repository
            .find_reference(name.as_str())
            .map_err(|_| unavailable())?;
        reference.delete().map_err(|_| unavailable())?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), PlatformError> {
    validate_directory(source)?;
    create_private_dir(destination)?;
    for entry in std::fs::read_dir(source).map_err(|_| unavailable())? {
        let entry = entry.map_err(|_| unavailable())?;
        let kind = entry.file_type().map_err(|_| unavailable())?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(unavailable());
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|_| unavailable())?;
            set_private_permissions(&target, false)?;
        } else {
            return Err(unavailable());
        }
    }
    Ok(())
}

fn validate_repository_tree(root: &Path) -> Result<(), PlatformError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        validate_directory(&directory)?;
        for entry in std::fs::read_dir(&directory).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            let kind = entry.file_type().map_err(|_| unavailable())?;
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                return Err(unavailable());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

fn harden_repository_tree(root: &Path) -> Result<(), PlatformError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        set_private_permissions(&directory, true)?;
        for entry in std::fs::read_dir(&directory).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            let kind = entry.file_type().map_err(|_| unavailable())?;
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                return Err(unavailable());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                set_private_permissions(&entry.path(), false)?;
            }
        }
    }
    Ok(())
}

fn directory_size(root: &Path, limit: u64) -> Result<u64, PlatformError> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            let kind = entry.file_type().map_err(|_| unavailable())?;
            if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                return Err(unavailable());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                total = total
                    .checked_add(entry.metadata().map_err(|_| unavailable())?.len())
                    .filter(|value| *value <= limit)
                    .ok_or_else(|| {
                        PlatformError::new(
                            ErrorCode::ResourceLimitExceeded,
                            "Artifact repository exceeds the configured byte limit",
                        )
                    })?;
            }
        }
    }
    Ok(total)
}

fn validate_directory(path: &Path) -> Result<(), PlatformError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.mode() & 0o022 != 0 {
            return Err(unavailable());
        }
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), PlatformError> {
    std::fs::create_dir(path).map_err(|_| unavailable())?;
    set_private_permissions(path, true)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), PlatformError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| unavailable())?;
    file.write_all(bytes).map_err(|_| unavailable())?;
    file.sync_all().map_err(|_| unavailable())
}

fn set_private_permissions(path: &Path, directory: bool) -> Result<(), PlatformError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if directory { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|_| unavailable())?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), PlatformError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())
}

fn valid_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_branch(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > 255
        || value.contains("..")
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'/' | b'.'))
    {
        Err(invalid())
    } else {
        Ok(())
    }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "Artifact Git identifier is invalid",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Artifact Git object was not found",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "Artifact Git repository is unavailable",
    )
}

#[cfg(test)]
mod tests;
