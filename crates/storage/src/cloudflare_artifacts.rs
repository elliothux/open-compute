//! Cloudflare Artifacts catalog, token, and immutable Worker-binding authority.

mod lifecycle;
mod model;

pub use model::{
    ARTIFACT_MAX_REPOSITORIES, ARTIFACT_MAX_TOKENS_PER_REPOSITORY,
    ARTIFACT_REPOSITORY_SCHEMA_VERSION, ArtifactNamespaceRecord, ArtifactRepositoryRecord,
    ArtifactRepositoryState, ArtifactTokenRecord, ArtifactTokenScope, AuthorizedArtifactBinding,
    NewArtifactRepository, NewArtifactToken, NewVersionArtifactBinding,
    VersionArtifactBindingRecord,
};
use model::{
    validate_description, validate_jurisdiction, validate_namespace_name, validate_ref_name,
    validate_repository_name,
};

use crate::ControlDb;
use open_compute_core::{
    AccountId, ArtifactRepoId, ArtifactTokenId, BindingId, ErrorCode, PlatformError, ResourceId,
    VersionId,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;
use subtle::ConstantTimeEq as _;

/// SQLite authority for Cloudflare Artifacts metadata.
#[derive(Clone, Copy, Debug)]
pub struct CloudflareArtifactsRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> CloudflareArtifactsRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read or atomically create a namespace with frozen Day 1 limits.
    pub fn ensure_namespace(
        &self,
        account_id: AccountId,
        name: &str,
        jurisdiction: Option<&str>,
        now_ms: i64,
    ) -> Result<ArtifactNamespaceRecord, PlatformError> {
        validate_namespace_name(name)?;
        validate_jurisdiction(jurisdiction)?;
        self.db.with_immediate(|tx| {
            if let Some(existing) = read_namespace_by_name(tx, account_id, name)? {
                if existing.jurisdiction.as_deref() != jurisdiction {
                    return Err(PlatformError::new(
                        ErrorCode::ResourceNameConflict,
                        "Artifact namespace jurisdiction conflicts with existing authority",
                    ));
                }
                return Ok(existing);
            }
            let id = ResourceId::generate();
            tx.execute(
                "INSERT INTO artifact_namespaces
                 (id, account_id, name, jurisdiction, max_repositories,
                  max_tokens_per_repository, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    id.to_string(),
                    account_id.to_string(),
                    name,
                    jurisdiction,
                    i64::from(ARTIFACT_MAX_REPOSITORIES),
                    i64::from(ARTIFACT_MAX_TOKENS_PER_REPOSITORY),
                    now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            read_namespace(tx, account_id, id)
        })
    }

    /// List account namespaces in stable name order.
    pub fn list_namespaces(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<ArtifactNamespaceRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, account_id, name, jurisdiction, max_repositories,
                            max_tokens_per_repository, created_at_ms, updated_at_ms
                     FROM artifact_namespaces WHERE account_id = ?1 ORDER BY name, id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([account_id.to_string()], map_namespace)
                .map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// Resolve one namespace by account and name.
    pub fn namespace_by_name(
        &self,
        account_id: AccountId,
        name: &str,
    ) -> Result<ArtifactNamespaceRecord, PlatformError> {
        self.db.with_read(|conn| {
            read_namespace_by_name(conn, account_id, name)?
                .ok_or_else(|| not_found("Artifact namespace was not found"))
        })
    }

    /// Resolve one namespace by immutable identity and account scope.
    pub fn namespace(
        &self,
        account_id: AccountId,
        id: ResourceId,
    ) -> Result<ArtifactNamespaceRecord, PlatformError> {
        self.db
            .with_read(|conn| read_namespace(conn, account_id, id))
    }

    /// Reserve repository identity before creating its bare Git directory.
    pub fn reserve_repository(
        &self,
        namespace: &ArtifactNamespaceRecord,
        request: NewArtifactRepository<'_>,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        validate_repository_name(request.name)?;
        validate_description(request.description)?;
        validate_ref_name(request.default_branch)?;
        if !matches!(
            request.initial_state,
            ArtifactRepositoryState::Creating
                | ArtifactRepositoryState::Importing
                | ArtifactRepositoryState::Forking
        ) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM artifact_repositories
                     WHERE namespace_id = ?1 AND state != 'tombstoned'",
                    [namespace.id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if count >= i64::from(namespace.max_repositories) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "Artifact namespace repository quota was exceeded",
                ));
            }
            let id = ArtifactRepoId::generate();
            tx.execute(
                "INSERT INTO artifact_repositories
                 (id, namespace_id, name, description, default_branch, state, read_only,
                  source, generation, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
                params![
                    id.to_string(),
                    namespace.id.to_string(),
                    request.name,
                    request.description,
                    request.default_branch,
                    request.initial_state.as_str(),
                    request.read_only,
                    request.source,
                    request.now_ms,
                ],
            )
            .map_err(|error| {
                if error.to_string().contains("UNIQUE") {
                    PlatformError::new(
                        ErrorCode::ResourceNameConflict,
                        "Artifact repository name already exists",
                    )
                } else {
                    db_error()
                }
            })?;
            read_repository(tx, id)
        })
    }

    /// Complete or fail a reserved repository initialization.
    pub fn finish_repository_create(
        &self,
        id: ArtifactRepoId,
        success: bool,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.finish_repository_state(id, ArtifactRepositoryState::Creating, success, now_ms)
    }

    /// Complete a reserved import and persist the remote's validated default branch.
    pub fn finish_repository_import(
        &self,
        id: ArtifactRepoId,
        default_branch: &str,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        validate_ref_name(default_branch)?;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories
                     SET state = 'ready', default_branch = ?2, updated_at_ms = ?3
                     WHERE id = ?1 AND state = 'importing'",
                    params![id.to_string(), default_branch, now_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            read_repository(tx, id)
        })
    }
    /// Resolve a ready repository by public namespace and repository names.
    pub fn repository_by_name(
        &self,
        account_id: AccountId,
        namespace: &str,
        name: &str,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT r.id, r.namespace_id, r.name, r.description, r.default_branch,
                        r.state, r.read_only, r.source, r.generation, r.created_at_ms,
                        r.updated_at_ms, r.last_push_at_ms, r.deleted_at_ms
                 FROM artifact_repositories r
                 JOIN artifact_namespaces n ON n.id = r.namespace_id
                 WHERE n.account_id = ?1 AND n.name = ?2 AND r.name = ?3
                   AND r.state != 'tombstoned'",
                params![account_id.to_string(), namespace, name],
                map_repository,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(|| not_found("Artifact repository was not found"))
        })
    }

    /// List repositories in one namespace.
    pub fn list_repositories(
        &self,
        namespace_id: ResourceId,
    ) -> Result<Vec<ArtifactRepositoryRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, namespace_id, name, description, default_branch, state,
                            read_only, source, generation, created_at_ms, updated_at_ms,
                            last_push_at_ms, deleted_at_ms
                     FROM artifact_repositories WHERE namespace_id = ?1 AND state = 'ready'
                     ORDER BY created_at_ms DESC, id DESC",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([namespace_id.to_string()], map_repository)
                .map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// List every non-tombstoned repository for startup reconciliation.
    pub fn list_live_repositories(&self) -> Result<Vec<ArtifactRepositoryRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, namespace_id, name, description, default_branch, state,
                            read_only, source, generation, created_at_ms, updated_at_ms,
                            last_push_at_ms, deleted_at_ms
                     FROM artifact_repositories WHERE state != 'tombstoned'
                     ORDER BY id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([], map_repository)
                .map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// Fence a repository before filesystem deletion.
    pub fn begin_delete_repository(
        &self,
        id: ArtifactRepoId,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories SET state = 'deleting', generation = generation + 1,
                            updated_at_ms = ?2 WHERE id = ?1 AND state IN ('ready', 'failed')",
                    params![id.to_string(), now_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(not_found("Artifact repository was not found or unavailable"));
            }
            read_repository(tx, id)
        })
    }

    /// Tombstone a fenced repository and revoke its tokens after directory removal.
    pub fn finish_delete_repository(
        &self,
        id: ArtifactRepoId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE artifact_repo_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?2)
                 WHERE repository_id = ?1",
                params![id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories
                     SET state = 'tombstoned', deleted_at_ms = ?2, updated_at_ms = ?2
                     WHERE id = ?1 AND state = 'deleting'",
                    params![id.to_string(), now_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            Ok(())
        })
    }

    /// Return a repository to ready when deletion admission times out before filesystem mutation.
    pub fn cancel_delete_repository(
        &self,
        id: ArtifactRepoId,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories
                     SET state = 'ready', generation = generation + 1, updated_at_ms = ?2
                     WHERE id = ?1 AND state = 'deleting'",
                    params![id.to_string(), now_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            read_repository(tx, id)
        })
    }

    /// Record a successful push without changing immutable identity fields.
    pub fn record_push(&self, id: ArtifactRepoId, now_ms: i64) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories SET last_push_at_ms = ?2, updated_at_ms = ?2
                     WHERE id = ?1 AND state = 'ready' AND read_only = 0",
                    params![id.to_string(), now_ms],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            Ok(())
        })
    }

    /// Persist one repository token digest and return its metadata.
    pub fn create_token(
        &self,
        repository: &ArtifactRepositoryRecord,
        request: NewArtifactToken<'_>,
    ) -> Result<ArtifactTokenRecord, PlatformError> {
        if request.expires_at_ms <= request.now_ms || request.max_tokens == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Artifact token expiry is invalid",
            ));
        }
        self.db.with_immediate(|tx| {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM artifact_repo_tokens
                     WHERE repository_id = ?1 AND revoked_at_ms IS NULL AND expires_at_ms > ?2",
                    params![repository.id.to_string(), request.now_ms],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if count >= i64::from(request.max_tokens) {
                return Err(PlatformError::new(ErrorCode::QuotaExceeded, "Artifact token quota was exceeded"));
            }
            tx.execute(
                "INSERT INTO artifact_repo_tokens
                 (id, repository_id, token_digest, scope, expires_at_ms, created_at_ms, revoked_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![request.id.to_string(), repository.id.to_string(), request.digest.as_slice(), request.scope.as_str(), request.expires_at_ms, request.now_ms],
            )
            .map_err(|_| db_error())?;
            read_token(tx, request.id)
        })
    }

    /// List repository token metadata without exposing digests.
    pub fn list_tokens(
        &self,
        repository_id: ArtifactRepoId,
    ) -> Result<Vec<ArtifactTokenRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, repository_id, scope, expires_at_ms, created_at_ms, revoked_at_ms
                 FROM artifact_repo_tokens WHERE repository_id = ?1 ORDER BY created_at_ms DESC, id DESC",
            ).map_err(|_| db_error())?;
            let rows = statement.query_map([repository_id.to_string()], map_token).map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// Revoke one token owned by the repository.
    pub fn revoke_token(
        &self,
        repository_id: ArtifactRepoId,
        id: ArtifactTokenId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let changed = self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE artifact_repo_tokens SET revoked_at_ms = ?3
                 WHERE id = ?1 AND repository_id = ?2 AND revoked_at_ms IS NULL",
                params![id.to_string(), repository_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())
        })?;
        if changed != 1 {
            return Err(not_found("Artifact token was not found"));
        }
        Ok(())
    }

    /// Revoke a token by namespace-scoped REST identity.
    pub fn revoke_namespace_token(
        &self,
        namespace_id: ResourceId,
        id: ArtifactTokenId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let changed = self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE artifact_repo_tokens SET revoked_at_ms = ?3
                 WHERE id = ?1 AND revoked_at_ms IS NULL
                   AND repository_id IN (
                     SELECT id FROM artifact_repositories WHERE namespace_id = ?2
                   )",
                params![id.to_string(), namespace_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())
        })?;
        if changed != 1 {
            return Err(not_found("Artifact token was not found"));
        }
        Ok(())
    }

    /// Validate an HMAC token digest and required scope against live authority.
    pub fn authenticate_token(
        &self,
        repository_id: ArtifactRepoId,
        digest: &[u8; 32],
        require_write: bool,
        now_ms: i64,
    ) -> Result<ArtifactTokenId, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT t.id, t.scope, t.token_digest FROM artifact_repo_tokens t
                     JOIN artifact_repositories r ON r.id = t.repository_id
                     WHERE t.repository_id = ?1 AND t.revoked_at_ms IS NULL
                       AND t.expires_at_ms > ?2 AND r.state = 'ready'",
                )
                .map_err(|_| db_error())?;
            let mut rows = statement
                .query(params![repository_id.to_string(), now_ms])
                .map_err(|_| db_error())?;
            let mut matched = None;
            while let Some(row) = rows.next().map_err(|_| db_error())? {
                let id = row
                    .get::<_, String>(0)
                    .map_err(|_| invariant())?
                    .parse::<ArtifactTokenId>()
                    .map_err(|_| invariant())?;
                let scope: String = row.get(1).map_err(|_| invariant())?;
                let stored_digest: Vec<u8> = row.get(2).map_err(|_| invariant())?;
                if stored_digest.len() != 32 {
                    return Err(invariant());
                }
                if bool::from(stored_digest.as_slice().ct_eq(digest.as_slice())) {
                    matched = Some((id, scope));
                }
            }
            let Some((id, scope)) = matched else {
                return Err(not_found("Artifact repository token is invalid"));
            };
            if require_write && scope != ArtifactTokenScope::Write.as_str() {
                return Err(PlatformError::new(
                    ErrorCode::BindingPermissionDenied,
                    "Artifact repository token is read-only",
                ));
            }
            Ok(id)
        })
    }

    /// Read immutable Artifact bindings for `RuntimeSource` reconstruction.
    pub fn version_bindings(
        &self,
        version_id: VersionId,
    ) -> Result<Vec<VersionArtifactBindingRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(
                "SELECT b.id, b.version_id, b.name, b.namespace_id, b.namespace_generation,
                        b.capability_version, b.permissions_json, b.descriptor_sha256, b.created_at_ms
                 FROM version_artifact_bindings b
                 JOIN artifact_namespaces n ON n.id = b.namespace_id
                 JOIN worker_versions v ON v.id = b.version_id
                 JOIN workers w ON w.id = v.worker_id
                 WHERE b.version_id = ?1 AND n.account_id = w.account_id ORDER BY b.name, b.id",
            ).map_err(|_| db_error())?;
            let rows = statement.query_map([version_id.to_string()], map_version_binding).map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// Authorize one private facade call from persisted immutable authority.
    pub fn authorize_binding(
        &self,
        binding_id: BindingId,
        version_id: VersionId,
        descriptor_sha256: &[u8; 32],
    ) -> Result<AuthorizedArtifactBinding, PlatformError> {
        self.db.with_read(|conn| {
            let binding = conn
                .query_row(
                    "SELECT b.id, b.version_id, b.name, b.namespace_id, b.namespace_generation,
                            b.capability_version, b.permissions_json, b.descriptor_sha256,
                            b.created_at_ms
                     FROM version_artifact_bindings b
                     JOIN worker_versions v ON v.id = b.version_id
                     JOIN workers w ON w.id = v.worker_id
                     JOIN artifact_namespaces n ON n.id = b.namespace_id
                     WHERE b.id = ?1 AND b.version_id = ?2 AND v.state = 'ready'
                       AND v.deleted_at_ms IS NULL AND n.account_id = w.account_id",
                    params![binding_id.to_string(), version_id.to_string()],
                    map_version_binding,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(|| not_found("Artifact binding was not found"))?;
            if binding.descriptor_sha256 != *descriptor_sha256
                || binding.namespace_generation != 1
                || binding.capability_version != 1
            {
                return Err(invariant());
            }
            let namespace = conn
                .query_row(
                    "SELECT id, account_id, name, jurisdiction, max_repositories,
                            max_tokens_per_repository, created_at_ms, updated_at_ms
                     FROM artifact_namespaces WHERE id = ?1",
                    [binding.namespace_id.to_string()],
                    map_namespace,
                )
                .map_err(|_| invariant())?;
            Ok(AuthorizedArtifactBinding { binding, namespace })
        })
    }
}

pub(crate) fn insert_version_bindings(
    tx: &Transaction<'_>,
    version_id: VersionId,
    bindings: &[NewVersionArtifactBinding],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for binding in bindings {
        let permissions = serde_json::to_vec(&binding.permissions).map_err(|_| invariant())?;
        tx.execute(
            "INSERT INTO version_artifact_bindings
             (id, version_id, name, namespace_id, namespace_generation, capability_version,
              permissions_json, descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                binding.id.to_string(),
                version_id.to_string(),
                binding.name,
                binding.namespace_id.to_string(),
                i64::try_from(binding.namespace_generation).map_err(|_| invariant())?,
                i64::from(binding.capability_version),
                permissions,
                binding.descriptor_sha256.as_slice(),
                now_ms
            ],
        )
        .map_err(|_| invariant())?;
    }
    Ok(())
}

fn read_namespace(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    id: ResourceId,
) -> Result<ArtifactNamespaceRecord, PlatformError> {
    conn.query_row(
        "SELECT id, account_id, name, jurisdiction, max_repositories,
                max_tokens_per_repository, created_at_ms, updated_at_ms
         FROM artifact_namespaces WHERE account_id = ?1 AND id = ?2",
        params![account_id.to_string(), id.to_string()],
        map_namespace,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(|| not_found("Artifact namespace was not found"))
}

fn read_namespace_by_name(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    name: &str,
) -> Result<Option<ArtifactNamespaceRecord>, PlatformError> {
    conn.query_row(
        "SELECT id, account_id, name, jurisdiction, max_repositories,
                max_tokens_per_repository, created_at_ms, updated_at_ms
         FROM artifact_namespaces WHERE account_id = ?1 AND name = ?2",
        params![account_id.to_string(), name],
        map_namespace,
    )
    .optional()
    .map_err(|_| db_error())
}

fn read_repository(
    conn: &rusqlite::Connection,
    id: ArtifactRepoId,
) -> Result<ArtifactRepositoryRecord, PlatformError> {
    conn.query_row(
        "SELECT id, namespace_id, name, description, default_branch, state, read_only,
                source, generation, created_at_ms, updated_at_ms, last_push_at_ms, deleted_at_ms
         FROM artifact_repositories WHERE id = ?1",
        [id.to_string()],
        map_repository,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(|| not_found("Artifact repository was not found"))
}

fn read_token(
    conn: &rusqlite::Connection,
    id: ArtifactTokenId,
) -> Result<ArtifactTokenRecord, PlatformError> {
    conn.query_row(
        "SELECT id, repository_id, scope, expires_at_ms, created_at_ms, revoked_at_ms
         FROM artifact_repo_tokens WHERE id = ?1",
        [id.to_string()],
        map_token,
    )
    .map_err(|_| db_error())
}

fn map_namespace(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactNamespaceRecord> {
    Ok(ArtifactNamespaceRecord {
        id: parse(row, 0)?,
        account_id: parse(row, 1)?,
        name: row.get(2)?,
        jurisdiction: row.get(3)?,
        max_repositories: positive(row.get(4)?)?,
        max_tokens_per_repository: positive(row.get(5)?)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn map_repository(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRepositoryRecord> {
    let state: String = row.get(5)?;
    Ok(ArtifactRepositoryRecord {
        id: parse(row, 0)?,
        namespace_id: parse(row, 1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        default_branch: row.get(4)?,
        state: state.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        read_only: row.get(6)?,
        source: row.get(7)?,
        generation: positive_u64(row.get(8)?)?,
        created_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
        last_push_at_ms: row.get(11)?,
        deleted_at_ms: row.get(12)?,
    })
}

fn map_token(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactTokenRecord> {
    let scope: String = row.get(2)?;
    Ok(ArtifactTokenRecord {
        id: parse(row, 0)?,
        repository_id: parse(row, 1)?,
        scope: scope.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        expires_at_ms: row.get(3)?,
        created_at_ms: row.get(4)?,
        revoked_at_ms: row.get(5)?,
    })
}

fn map_version_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionArtifactBindingRecord> {
    let permissions: Vec<u8> = row.get(6)?;
    let digest: Vec<u8> = row.get(7)?;
    Ok(VersionArtifactBindingRecord {
        id: parse(row, 0)?,
        version_id: parse(row, 1)?,
        name: row.get(2)?,
        namespace_id: parse(row, 3)?,
        namespace_generation: positive_u64(row.get(4)?)?,
        capability_version: positive(row.get(5)?)?,
        permissions: serde_json::from_slice(&permissions)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        descriptor_sha256: digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(8)?,
    })
}

fn parse<T: FromStr>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T> {
    row.get::<_, String>(index)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
fn positive(value: i64) -> rusqlite::Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(rusqlite::Error::InvalidQuery)
}
fn positive_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(rusqlite::Error::InvalidQuery)
}
fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| invariant())
}

fn not_found(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, message)
}
fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::MigrationFailed,
        "Artifact catalog operation failed",
    )
}
fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Artifact catalog invariant failed",
    )
}

#[cfg(test)]
mod tests;
