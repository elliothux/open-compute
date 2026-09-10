//! Shared Cloudflare Artifacts control and Git data-plane service.

pub(crate) mod leases;

use open_compute_artifacts::GitRepositoryStore;
use open_compute_core::{
    AccountId, ArtifactRepoId, ArtifactTokenId, ArtifactsConfig, ErrorCode, PlatformError,
};
use open_compute_storage::{
    ArtifactNamespaceRecord, ArtifactRepositoryRecord, ArtifactRepositoryState,
    ArtifactTokenRecord, ArtifactTokenScope, CloudflareArtifactsRepository, NewArtifactRepository,
    NewArtifactToken, PlatformStorage,
};
use rand::TryRngCore as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use leases::{RepositoryLease, RepositoryLeases};

const TOKEN_PREFIX: &str = "art_v1_";

#[derive(Clone, Debug)]
pub(crate) struct ArtifactApiState {
    storage: Arc<PlatformStorage>,
    git: GitRepositoryStore,
    config: ArtifactsConfig,
    requests: Arc<Semaphore>,
    leases: Arc<RepositoryLeases>,
}

#[derive(Debug)]
pub(crate) struct IssuedArtifactToken {
    pub(crate) record: ArtifactTokenRecord,
    pub(crate) plaintext: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CreateRepositoryRequest<'a> {
    pub(crate) name: &'a str,
    pub(crate) description: &'a str,
    pub(crate) default_branch: &'a str,
    pub(crate) read_only: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ImportRepositoryRequest {
    pub(crate) account: AccountId,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) remote: String,
    pub(crate) branch: Option<String>,
    pub(crate) depth: Option<u32>,
    pub(crate) description: String,
    pub(crate) read_only: bool,
    pub(crate) now_ms: i64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ForkRepositoryRequest<'a> {
    pub(crate) source_name: &'a str,
    pub(crate) target_name: &'a str,
    pub(crate) description: Option<&'a str>,
    pub(crate) read_only: Option<bool>,
    pub(crate) default_branch_only: bool,
}

impl ArtifactApiState {
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        config: ArtifactsConfig,
    ) -> Result<Self, PlatformError> {
        let git = GitRepositoryStore::open(
            storage.data_dir().artifact_git_dir(),
            storage.data_dir().artifact_quarantine_dir(),
            config.max_object_response_bytes,
        )?;
        let permits = usize::try_from(config.max_concurrent_requests).map_err(|_| {
            PlatformError::new(ErrorCode::LimitInvalid, "Artifacts concurrency is invalid")
        })?;
        let state = Self {
            storage,
            git,
            config,
            requests: Arc::new(Semaphore::new(permits)),
            leases: Arc::new(RepositoryLeases::default()),
        };
        state.git.cleanup_quarantine()?;
        state.reconcile(open_compute_core::wall_time_ms())?;
        Ok(state)
    }

    pub(crate) fn admit_git(&self) -> Result<OwnedSemaphorePermit, PlatformError> {
        self.requests.clone().try_acquire_owned().map_err(|_| {
            PlatformError::new(ErrorCode::AdmissionBusy, "Artifacts Git service is busy")
        })
    }

    pub(crate) const fn max_request_bytes(&self) -> u64 {
        self.config.max_request_bytes
    }

    pub(crate) fn admit_git_mutation(
        &self,
        repository: ArtifactRepoId,
    ) -> Result<open_compute_core::AdmissionReservation, PlatformError> {
        let maximum_existing = self
            .config
            .max_repository_bytes
            .saturating_sub(self.config.max_request_bytes);
        self.git.repository_size(repository, maximum_existing)?;
        self.storage.reserve_mutation(self.config.max_request_bytes)
    }

    pub(crate) const fn git(&self) -> &GitRepositoryStore {
        &self.git
    }

    pub(crate) fn create_repository(
        &self,
        account: AccountId,
        namespace: &str,
        request: CreateRepositoryRequest<'_>,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        let _disk = self.storage.reserve_mutation(1024 * 1024)?;
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        let namespace = catalog.namespace_by_name(account, namespace)?;
        let record = catalog.reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: request.name,
                description: request.description,
                default_branch: request.default_branch,
                read_only: request.read_only,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms,
            },
        )?;
        match self.git.initialize(record.id, request.default_branch) {
            Ok(_) => catalog.finish_repository_create(record.id, true, now_ms),
            Err(error) => {
                let _ = catalog.finish_repository_create(record.id, false, now_ms);
                Err(error)
            }
        }
    }

    pub(crate) fn create_namespace(
        &self,
        account: AccountId,
        namespace: &str,
        now_ms: i64,
    ) -> Result<ArtifactNamespaceRecord, PlatformError> {
        CloudflareArtifactsRepository::new(self.storage.db())
            .ensure_namespace(account, namespace, None, now_ms)
    }

    pub(crate) async fn import_repository(
        &self,
        mut request: ImportRepositoryRequest,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        request.remote = GitRepositoryStore::validate_import_url(&request.remote)?;
        let state = self.clone();
        tokio::task::spawn_blocking(move || state.import_repository_blocking(&request))
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::ResourceUnavailable,
                    "Artifact import task failed",
                )
            })?
    }

    fn import_repository_blocking(
        &self,
        request: &ImportRepositoryRequest,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        let _disk = self
            .storage
            .reserve_mutation(self.config.max_repository_bytes)?;
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        let namespace = catalog.namespace_by_name(request.account, &request.namespace)?;
        let record = catalog.reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: &request.name,
                description: &request.description,
                default_branch: request.branch.as_deref().unwrap_or("main"),
                read_only: request.read_only,
                source: Some(&request.remote),
                initial_state: ArtifactRepositoryState::Importing,
                now_ms: request.now_ms,
            },
        )?;
        let imported = self
            .git
            .import_public_https(
                record.id,
                &request.remote,
                request.branch.as_deref(),
                request.depth,
                self.config.max_repository_bytes,
                Duration::from_millis(self.config.import_timeout_ms),
            )
            .and_then(|_| self.git.default_branch(record.id));
        match imported {
            Ok(default_branch) => {
                catalog.finish_repository_import(record.id, &default_branch, request.now_ms)
            }
            Err(error) => {
                let _ = self.git.discard_unpublished(record.id);
                let _ = catalog.fail_repository(
                    record.id,
                    ArtifactRepositoryState::Importing,
                    request.now_ms,
                );
                Err(error)
            }
        }
    }

    pub(crate) fn fork_repository(
        &self,
        account: AccountId,
        namespace: &str,
        request: ForkRepositoryRequest<'_>,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        let source = catalog.repository_by_name(account, namespace, request.source_name)?;
        require_ready(&source)?;
        let _source_lease = self.leases.acquire(source.id)?;
        let source = catalog.repository_by_name(account, namespace, request.source_name)?;
        require_ready(&source)?;
        let source_bytes = self
            .git
            .repository_size(source.id, self.config.max_repository_bytes)?;
        let _disk = self.storage.reserve_mutation(source_bytes.max(1))?;
        let namespace_record = catalog.namespace_by_name(account, namespace)?;
        let fork_source = format!("artifacts:{namespace}/{}", request.source_name);
        let target = catalog.reserve_repository(
            &namespace_record,
            NewArtifactRepository {
                name: request.target_name,
                description: request.description.unwrap_or(&source.description),
                default_branch: &source.default_branch,
                read_only: request.read_only.unwrap_or(source.read_only),
                source: Some(&fork_source),
                initial_state: ArtifactRepositoryState::Forking,
                now_ms,
            },
        )?;
        match self.git.fork(
            source.id,
            target.id,
            request.default_branch_only,
            &source.default_branch,
            self.config.max_repository_bytes,
        ) {
            Ok(_) => catalog.finish_repository_fork(target.id, true, now_ms),
            Err(error) => {
                let _ = catalog.finish_repository_fork(target.id, false, now_ms);
                Err(error)
            }
        }
    }

    pub(crate) fn object_count(&self, repository: ArtifactRepoId) -> Result<usize, PlatformError> {
        let _lease = self.leases.acquire(repository)?;
        self.git.object_count(repository)
    }

    pub(crate) fn read_object(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        oid: &str,
    ) -> Result<open_compute_artifacts::GitObject, PlatformError> {
        let (repository, _lease) = self.repository_with_lease(account, namespace, repository)?;
        self.git.read_object(repository.id, oid)
    }

    pub(crate) fn read_file(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        revision: &str,
        path: &str,
    ) -> Result<open_compute_artifacts::GitObject, PlatformError> {
        let (repository, _lease) = self.repository_with_lease(account, namespace, repository)?;
        self.git.read_file(repository.id, revision, path)
    }

    pub(crate) fn commit_log(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        revision: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<open_compute_artifacts::GitObject>, PlatformError> {
        let (repository, _lease) = self.repository_with_lease(account, namespace, repository)?;
        self.git.commit_log(
            repository.id,
            revision.unwrap_or(&repository.default_branch),
            offset,
            limit,
        )
    }

    pub(crate) fn delete_repository(
        &self,
        account: AccountId,
        namespace: &str,
        name: &str,
        now_ms: i64,
    ) -> Result<ArtifactRepoId, PlatformError> {
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        let record = catalog.repository_by_name(account, namespace, name)?;
        catalog.begin_delete_repository(record.id, now_ms)?;
        if let Err(error) = self.leases.drain(
            record.id,
            Duration::from_millis(self.config.lease_drain_timeout_ms),
        ) {
            catalog.cancel_delete_repository(record.id, now_ms)?;
            return Err(error);
        }
        self.git.delete(record.id)?;
        catalog.finish_delete_repository(record.id, now_ms)?;
        Ok(record.id)
    }

    pub(crate) fn repository(
        &self,
        account: AccountId,
        namespace: &str,
        name: &str,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        let record = CloudflareArtifactsRepository::new(self.storage.db())
            .repository_by_name(account, namespace, name)?;
        require_ready(&record)?;
        self.git.verify(record.id)?;
        Ok(record)
    }

    pub(crate) fn repository_for_binding(
        &self,
        account: AccountId,
        namespace: &str,
        name: &str,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        let record = CloudflareArtifactsRepository::new(self.storage.db())
            .repository_by_name(account, namespace, name)?;
        if record.state == ArtifactRepositoryState::Ready {
            self.git.verify(record.id)?;
        }
        Ok(record)
    }

    pub(crate) fn repository_with_lease(
        &self,
        account: AccountId,
        namespace: &str,
        name: &str,
    ) -> Result<(ArtifactRepositoryRecord, RepositoryLease), PlatformError> {
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        let first = catalog.repository_by_name(account, namespace, name)?;
        require_ready(&first)?;
        let lease = self.leases.acquire(first.id)?;
        let record = catalog.repository_by_name(account, namespace, name)?;
        if record.id != first.id {
            return Err(PlatformError::new(
                ErrorCode::ResourceUnavailable,
                "Artifact repository identity changed during admission",
            ));
        }
        require_ready(&record)?;
        self.git.verify(record.id)?;
        Ok((record, lease))
    }

    pub(crate) fn list_namespaces(
        &self,
        account: AccountId,
    ) -> Result<Vec<ArtifactNamespaceRecord>, PlatformError> {
        CloudflareArtifactsRepository::new(self.storage.db()).list_namespaces(account)
    }

    pub(crate) fn namespace(
        &self,
        account: AccountId,
        name: &str,
    ) -> Result<ArtifactNamespaceRecord, PlatformError> {
        CloudflareArtifactsRepository::new(self.storage.db()).namespace_by_name(account, name)
    }

    pub(crate) fn list_repositories(
        &self,
        account: AccountId,
        namespace: &str,
    ) -> Result<Vec<ArtifactRepositoryRecord>, PlatformError> {
        let namespace = self.namespace(account, namespace)?;
        CloudflareArtifactsRepository::new(self.storage.db()).list_repositories(namespace.id)
    }

    pub(crate) fn remote(&self, namespace: &str, repository: &str) -> String {
        format!(
            "{}/git/{namespace}/{repository}.git",
            self.config.public_origin.trim_end_matches('/')
        )
    }

    pub(crate) fn issue_token(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        scope: ArtifactTokenScope,
        ttl_seconds: Option<u32>,
        now_ms: i64,
    ) -> Result<IssuedArtifactToken, PlatformError> {
        let repo = self.repository(account, namespace, repository)?;
        let namespace = self.namespace(account, namespace)?;
        let ttl = ttl_seconds.unwrap_or(self.config.token_ttl_seconds);
        if ttl < 60 || ttl > self.config.max_token_ttl_seconds {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Artifact token TTL is invalid",
            ));
        }
        let id = ArtifactTokenId::generate();
        let mut random = [0u8; 20];
        rand::rngs::OsRng.try_fill_bytes(&mut random).map_err(|_| {
            PlatformError::new(
                ErrorCode::ResourceUnavailable,
                "Artifact token generation failed",
            )
        })?;
        let expires_at_ms = now_ms.checked_add(i64::from(ttl) * 1_000).ok_or_else(|| {
            PlatformError::new(ErrorCode::LimitInvalid, "Artifact token expiry is invalid")
        })?;
        let secret = format!("{TOKEN_PREFIX}{}", hex::encode(random));
        let plaintext = format!("{secret}?expires={}", expires_at_ms / 1_000);
        let digest = self
            .storage
            .crypto()
            .sign_artifact_repository_token(secret.as_bytes());
        let record = CloudflareArtifactsRepository::new(self.storage.db()).create_token(
            &repo,
            NewArtifactToken {
                id,
                digest: &digest,
                scope,
                expires_at_ms,
                now_ms,
                max_tokens: namespace.max_tokens_per_repository,
            },
        )?;
        Ok(IssuedArtifactToken { record, plaintext })
    }

    pub(crate) fn issue_initial_token(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        now_ms: i64,
    ) -> Result<IssuedArtifactToken, PlatformError> {
        match self.issue_token(
            account,
            namespace,
            repository,
            ArtifactTokenScope::Write,
            None,
            now_ms,
        ) {
            Ok(token) => Ok(token),
            Err(error) => {
                Err(self.abandon_created_repository(account, namespace, repository, now_ms, error))
            }
        }
    }

    pub(crate) fn abandon_created_repository(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        now_ms: i64,
        cause: PlatformError,
    ) -> PlatformError {
        match self.delete_repository(account, namespace, repository, now_ms) {
            Ok(_) => cause,
            Err(cleanup) => cleanup,
        }
    }

    pub(crate) fn list_tokens(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
    ) -> Result<Vec<ArtifactTokenRecord>, PlatformError> {
        let repository = self.repository(account, namespace, repository)?;
        CloudflareArtifactsRepository::new(self.storage.db()).list_tokens(repository.id)
    }

    pub(crate) fn revoke_namespace_token(
        &self,
        account: AccountId,
        namespace: &str,
        token: ArtifactTokenId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let namespace = self.namespace(account, namespace)?;
        CloudflareArtifactsRepository::new(self.storage.db()).revoke_namespace_token(
            namespace.id,
            token,
            now_ms,
        )
    }

    pub(crate) fn authenticate_git(
        &self,
        repository: &ArtifactRepositoryRecord,
        plaintext: &str,
        require_write: bool,
        now_ms: i64,
    ) -> Result<ArtifactTokenId, PlatformError> {
        let secret = token_secret(plaintext)?;
        let digest = self
            .storage
            .crypto()
            .sign_artifact_repository_token(secret.as_bytes());
        CloudflareArtifactsRepository::new(self.storage.db()).authenticate_token(
            repository.id,
            &digest,
            require_write,
            now_ms,
        )
    }

    pub(crate) fn revoke_token_value(
        &self,
        account: AccountId,
        namespace: &str,
        repository: &str,
        token_or_id: &str,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let repository_record = self.repository(account, namespace, repository)?;
        let id = if token_or_id.starts_with(TOKEN_PREFIX) {
            self.authenticate_git(&repository_record, token_or_id, false, now_ms)?
        } else {
            token_or_id.parse().map_err(|_| invalid_token())?
        };
        match CloudflareArtifactsRepository::new(self.storage.db()).revoke_token(
            repository_record.id,
            id,
            now_ms,
        ) {
            Ok(()) => Ok(true),
            Err(error) if error.code() == ErrorCode::ResourceNotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn record_push(
        &self,
        repository: ArtifactRepoId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        CloudflareArtifactsRepository::new(self.storage.db()).record_push(repository, now_ms)
    }

    fn reconcile(&self, now_ms: i64) -> Result<(), PlatformError> {
        let catalog = CloudflareArtifactsRepository::new(self.storage.db());
        for repository in catalog.list_live_repositories()? {
            match repository.state {
                ArtifactRepositoryState::Ready => {
                    if self.git.verify(repository.id).is_err() {
                        self.git.quarantine_corrupt(repository.id)?;
                        catalog.fail_repository(
                            repository.id,
                            ArtifactRepositoryState::Ready,
                            now_ms,
                        )?;
                    }
                }
                ArtifactRepositoryState::Creating => {
                    if self.git.verify(repository.id).is_ok() {
                        catalog.finish_repository_create(repository.id, true, now_ms)?;
                    } else {
                        self.git.quarantine_corrupt(repository.id)?;
                        catalog.finish_repository_create(repository.id, false, now_ms)?;
                    }
                }
                ArtifactRepositoryState::Importing => {
                    if let Ok(default_branch) = self.git.default_branch(repository.id) {
                        catalog.finish_repository_import(repository.id, &default_branch, now_ms)?;
                    } else {
                        self.git.quarantine_corrupt(repository.id)?;
                        catalog.fail_repository(
                            repository.id,
                            ArtifactRepositoryState::Importing,
                            now_ms,
                        )?;
                    }
                }
                ArtifactRepositoryState::Forking => {
                    if self.git.verify(repository.id).is_ok() {
                        catalog.finish_repository_fork(repository.id, true, now_ms)?;
                    } else {
                        self.git.quarantine_corrupt(repository.id)?;
                        catalog.finish_repository_fork(repository.id, false, now_ms)?;
                    }
                }
                ArtifactRepositoryState::Deleting => {
                    self.git.delete(repository.id)?;
                    catalog.finish_delete_repository(repository.id, now_ms)?;
                }
                ArtifactRepositoryState::Failed => {
                    self.git.quarantine_corrupt(repository.id)?;
                }
                ArtifactRepositoryState::Tombstoned => return Err(invariant()),
            }
        }
        Ok(())
    }
}

fn token_secret(plaintext: &str) -> Result<&str, PlatformError> {
    let secret = match plaintext.split_once("?expires=") {
        Some((secret, expiry)) if expiry.parse::<u64>().is_ok() => secret,
        None => plaintext,
        Some(_) => return Err(invalid_token()),
    };
    let hex = secret
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(invalid_token)?;
    if hex.len() != 40
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_token());
    }
    Ok(secret)
}

fn require_ready(record: &ArtifactRepositoryRecord) -> Result<(), PlatformError> {
    if record.state == ArtifactRepositoryState::Ready {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::ResourceNotReady,
            "Artifact repository is not ready",
        ))
    }
}

fn invalid_token() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Artifact repository token is invalid",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Artifact repository reconciliation invariant failed",
    )
}

#[cfg(test)]
mod tests;
