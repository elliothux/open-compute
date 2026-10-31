use super::*;

/// One verified content-addressed blob owned by an authenticated registry repository.
#[derive(Clone, Debug)]
pub struct ContainerRegistryBlob {
    /// Artifact identity, also the OCI blob digest.
    pub sha256: [u8; 32],
    /// Exact immutable artifact length.
    pub size: u64,
}

impl ContainerAuthority<'_> {
    /// Enumerate registered spool identities for orphan recovery without trusting filenames.
    pub fn registry_upload_ids(
        &self,
        instance: InstanceId,
    ) -> Result<Vec<ResourceId>, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn, instance)?;
            let mut stmt = conn
                .prepare("SELECT id FROM container_registry_uploads ORDER BY id")
                .map_err(|_| invariant())?;
            let ids = stmt
                .query_map([], |r| r.get::<_, ResourceId>(0))
                .map_err(|_| invariant())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| invariant())?;
            Ok(ids)
        })
    }
    /// Atomically expire abandoned uploads, credentials and uncommitted blob staging references.
    pub fn expire_registry_staging(
        &self,
        instance: InstanceId,
        now: i64,
    ) -> Result<Vec<ResourceId>, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance)?;
            let ids = {
                let mut stmt = tx
                    .prepare("SELECT id FROM container_registry_uploads WHERE expires_at_ms<=?1")
                    .map_err(|_| invariant())?;
                stmt.query_map([now], |r| r.get::<_, ResourceId>(0))
                    .map_err(|_| invariant())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|_| invariant())?
            };
            tx.execute(
                "DELETE FROM container_registry_uploads WHERE expires_at_ms<=?1",
                [now],
            )
            .map_err(|_| invariant())?;
            tx.execute(
                "DELETE FROM container_registry_tokens WHERE expires_at_ms<=?1",
                [now],
            )
            .map_err(|_| invariant())?;
            tx.execute(
                "DELETE FROM container_registry_blobs WHERE expires_at_ms<=?1",
                [now],
            )
            .map_err(|_| invariant())?;
            Ok(ids)
        })
    }
    /// Mount an already verified blob only between repositories owned by this instance.
    pub fn mount_registry_blob(
        &self,
        instance: InstanceId,
        source: &str,
        target: &str,
        sha: &[u8; 32],
        now: i64,
    ) -> Result<bool, PlatformError> {
        validate_repository(instance, source)?;
        validate_repository(instance, target)?;
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM container_registry_blobs WHERE repository=?1 AND artifact_sha256=?2 AND expires_at_ms>?3)",params![source,sha,now],|r|r.get(0)).map_err(|_|invariant())?;
            if present {tx.execute("INSERT INTO container_registry_blobs(repository,artifact_sha256,artifact_size,created_at_ms,expires_at_ms) SELECT ?1,artifact_sha256,artifact_size,?3,expires_at_ms FROM container_registry_blobs WHERE repository=?2 AND artifact_sha256=?4 ON CONFLICT(repository,artifact_sha256) DO NOTHING",params![target,source,now,sha]).map_err(|_|invariant())?;}
            Ok(present)
        })
    }
    /// Persist the exact admitted manifest bytes for standard registry reads.
    pub fn publish_registry_manifest(
        &self,
        instance: InstanceId,
        repository: &str,
        manifest: &[u8; 32],
        content: &[u8],
    ) -> Result<(), PlatformError> {
        use sha2::{Digest as _, Sha256};
        validate_repository(instance, repository)?;
        if content.is_empty()
            || content.len() > 1048576
            || Sha256::digest(content).as_slice() != manifest
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let value:serde_json::Value=serde_json::from_slice(content).map_err(|_|invariant())?;
            let descriptors=value.get("layers").and_then(serde_json::Value::as_array).ok_or_else(invariant)?;
            let config=value.get("config").ok_or_else(invariant)?;
            for descriptor in std::iter::once(config).chain(descriptors.iter()) {
                let digest=descriptor.get("digest").and_then(serde_json::Value::as_str).and_then(|d|d.strip_prefix("sha256:")).ok_or_else(invariant)?;
                let digest=hex::decode(digest).map_err(|_|invariant())?;
                if digest.len()!=32 {return Err(invariant());}
                let present=tx.execute("UPDATE container_registry_blobs SET expires_at_ms=?3 WHERE repository=?1 AND artifact_sha256=?2",params![repository,digest,i64::MAX]).map_err(|_|invariant())?;
                if present!=1 {return Err(not_found());}
            }
            tx.execute("INSERT INTO container_registry_manifests(repository,manifest_sha256,content) VALUES(?1,?2,?3) ON CONFLICT(repository,manifest_sha256) DO NOTHING",params![repository,manifest,content]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Read and verify immutable manifest bytes without repairing corrupt persisted data.
    pub fn registry_manifest(
        &self,
        instance: InstanceId,
        repository: &str,
        manifest: &[u8; 32],
    ) -> Result<Vec<u8>, PlatformError> {
        use sha2::{Digest as _, Sha256};
        validate_repository(instance, repository)?;
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let content:Vec<u8>=conn.query_row("SELECT content FROM container_registry_manifests WHERE repository=?1 AND manifest_sha256=?2",params![repository,manifest],|row|row.get(0)).optional().map_err(|_|invariant())?.ok_or_else(not_found)?;
            if Sha256::digest(&content).as_slice()!=manifest {return Err(invariant());} Ok(content)
        })
    }
    /// Persist only a short-lived registry password hash and its explicit permissions.
    pub fn issue_registry_token(
        &self,
        instance: InstanceId,
        hash: &[u8; 32],
        push: bool,
        pull: bool,
        expires: i64,
    ) -> Result<(), PlatformError> {
        if !push && !pull {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            tx.execute("INSERT INTO container_registry_tokens(token_sha256,repository_scope,can_push,can_pull,expires_at_ms) VALUES(?1,?2,?3,?4,?5)",params![hash,instance,push,pull,expires]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Authenticate the installation, repository scope, permission and expiry together.
    pub fn authenticate_registry(
        &self,
        instance: InstanceId,
        hash: &[u8; 32],
        repository: &str,
        push: bool,
        now: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let scope = repository.split('/').next().ok_or_else(invariant)?;
            if scope != instance.as_str() { return Ok(false); }
            conn.query_row("SELECT EXISTS(SELECT 1 FROM container_registry_tokens WHERE token_sha256=?1 AND repository_scope=?2 AND expires_at_ms>?3 AND CASE WHEN ?4 THEN can_push ELSE can_pull END=1)", params![hash,scope,now,push],|row|row.get(0)).map_err(|_|invariant())
        })
    }

    /// Register an upload identity before accepting any bytes.
    pub fn create_registry_upload(
        &self,
        instance: InstanceId,
        id: ResourceId,
        repository: &str,
        expires: i64,
    ) -> Result<(), PlatformError> {
        validate_repository(instance, repository)?;
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            tx.execute("INSERT INTO container_registry_uploads(id,repository,size,busy,expires_at_ms) VALUES(?1,?2,0,0,?3)",params![id,repository,expires]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Claim exactly one append; a crash leaves the upload fenced until expiry.
    pub fn claim_registry_upload(
        &self,
        instance: InstanceId,
        id: ResourceId,
        repository: &str,
        now: i64,
    ) -> Result<u64, PlatformError> {
        validate_repository(instance, repository)?;
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed = tx.execute("UPDATE container_registry_uploads SET busy=1 WHERE id=?1 AND repository=?2 AND expires_at_ms>?3 AND busy=0",params![id,repository,now]).map_err(|_|invariant())?;
            if changed!=1 { return Err(not_found()); }
            tx.query_row("SELECT size FROM container_registry_uploads WHERE id=?1",[id],|row|row.get(0)).map_err(|_|invariant())
        })
    }

    /// Commit a completed append after its file has been flushed.
    pub fn finish_registry_append(
        &self,
        instance: InstanceId,
        id: ResourceId,
        prior: u64,
        size: u64,
    ) -> Result<(), PlatformError> {
        if size < prior || size > i64::MAX as u64 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed=tx.execute("UPDATE container_registry_uploads SET size=?3,busy=0 WHERE id=?1 AND busy=1 AND size=?2",params![id,prior,size]).map_err(|_|invariant())?;
            if changed!=1 {return Err(invariant());} Ok(())
        })
    }

    /// Atomically protect a verified blob from GC and complete its upload.
    pub fn commit_registry_blob(
        &self,
        instance: InstanceId,
        id: ResourceId,
        repository: &str,
        blob: &ContainerRegistryBlob,
        now: i64,
    ) -> Result<(), PlatformError> {
        validate_repository(instance, repository)?;
        if blob.size == 0 || blob.size > i64::MAX as u64 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            let changed=tx.execute("DELETE FROM container_registry_uploads WHERE id=?1 AND repository=?2 AND size=?3 AND busy=1",params![id,repository,blob.size]).map_err(|_|invariant())?;
            if changed!=1 {return Err(invariant());}
            tx.execute("INSERT INTO container_registry_blobs(repository,artifact_sha256,artifact_size,created_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(repository,artifact_sha256) DO NOTHING",params![repository,blob.sha256,blob.size,now,now.checked_add(900_000).ok_or_else(invariant)?]).map_err(|_|invariant())?;
            let same:bool=tx.query_row("SELECT artifact_size=?3 FROM container_registry_blobs WHERE repository=?1 AND artifact_sha256=?2",params![repository,blob.sha256,blob.size],|row|row.get(0)).map_err(|_|invariant())?;
            if !same {return Err(invariant());} Ok(())
        })
    }

    /// Read a repository's verified staged blobs without changing persisted bytes.
    pub fn registry_blobs(
        &self,
        instance: InstanceId,
        repository: &str,
    ) -> Result<Vec<ContainerRegistryBlob>, PlatformError> {
        validate_repository(instance, repository)?;
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let mut stmt=conn.prepare("SELECT artifact_sha256,artifact_size FROM container_registry_blobs WHERE repository=?1 ORDER BY artifact_sha256").map_err(|_|invariant())?;
            stmt.query_map([repository],|row|{let sha:Vec<u8>=row.get(0)?;Ok(ContainerRegistryBlob{sha256:sha.try_into().map_err(|_|rusqlite::Error::InvalidQuery)?,size:row.get(1)?})}).map_err(|_|invariant())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|_|invariant())
        })
    }

    /// Bind a mutable producer tag once its complete immutable graph has been admitted.
    pub fn publish_registry_tag(
        &self,
        instance: InstanceId,
        repository: &str,
        tag: &str,
        manifest: &[u8; 32],
        now: i64,
    ) -> Result<(), PlatformError> {
        validate_repository(instance, repository)?;
        validate_registry_tag(tag)?;
        self.db.with_immediate(|tx| {
            require_instance(tx,instance)?;
            tx.execute("INSERT INTO container_registry_tags(repository,tag,manifest_sha256,updated_at_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(repository,tag) DO UPDATE SET manifest_sha256=excluded.manifest_sha256,updated_at_ms=excluded.updated_at_ms",params![repository,tag,manifest,now]).map_err(|_|invariant())?;
            Ok(())
        })
    }

    /// Resolve a repository-scoped producer tag to an admitted immutable graph.
    pub fn registry_tag(
        &self,
        instance: InstanceId,
        repository: &str,
        tag: &str,
    ) -> Result<[u8; 32], PlatformError> {
        validate_repository(instance, repository)?;
        validate_registry_tag(tag)?;
        self.db.with_read(|conn| {
            require_instance(conn,instance)?;
            let bytes:Vec<u8>=conn.query_row("SELECT manifest_sha256 FROM container_registry_tags WHERE repository=?1 AND tag=?2",params![repository,tag],|row|row.get(0)).optional().map_err(|_|invariant())?.ok_or_else(not_found)?;
            bytes.try_into().map_err(|_|invariant())
        })
    }
}

fn validate_repository(instance: InstanceId, value: &str) -> Result<(), PlatformError> {
    if value.len() > 255
        || value.split('/').next() != Some(instance.as_str())
        || value.split('/').count() < 2
        || value.split('/').skip(1).any(|part| {
            part.is_empty()
                || !part
                    .as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || !part
                    .as_bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || !part.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'-'
                        || b == b'_'
                        || b == b'.'
                })
        })
    {
        return Err(invariant());
    }
    Ok(())
}
/// Validate a producer tag before artifact admission or registry authority writes.
///
/// Tags contain at most 128 ASCII letters, digits, dots, underscores or hyphens,
/// and cannot begin with a dot or hyphen.
pub fn validate_registry_tag(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > 128
        || value.starts_with('.')
        || value.starts_with('-')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invariant());
    }
    Ok(())
}
