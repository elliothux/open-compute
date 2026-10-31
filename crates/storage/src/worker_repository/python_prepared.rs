//! Immutable prepared-snapshot authority and retained object references.

use super::*;

/// Immutable ciphertext object published only after successful Python preparation.
#[derive(Clone, Eq, PartialEq)]
pub struct PythonPreparedArtifactRecord {
    /// Version whose module/env inputs were prepared.
    pub version_id: VersionId,
    /// Domain-separated identity digest derived by the Workers owner.
    pub prepared_identity_sha256: [u8; 32],
    /// Canonical identity metadata; contains digests and IDs, never snapshot bytes.
    pub identity_json: Vec<u8>,
    /// Verified ciphertext object digest from the immutable artifact store.
    pub artifact_sha256: [u8; 32],
    /// Exact ciphertext object size, including its bounded envelope.
    pub artifact_size: u64,
    /// Time the object reference was committed.
    pub created_at_ms: i64,
}

impl std::fmt::Debug for PythonPreparedArtifactRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PythonPreparedArtifactRecord")
            .field("version_id", &self.version_id)
            .field("artifact_sha256", &hex::encode(self.artifact_sha256))
            .field("artifact_size", &self.artifact_size)
            .finish_non_exhaustive()
    }
}

impl WorkerRepository<'_> {
    /// Commit an already verified encrypted snapshot for a validating Python Version.
    ///
    /// The caller holds its `ArtifactStore` upload/GC reservation across this transaction.
    /// Existing records must be reused by recovery; this operation never overwrites one.
    pub fn publish_python_prepared(
        &self,
        instance: InstanceId,
        worker: WorkerId,
        artifact: &PythonPreparedArtifactRecord,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_live_worker(tx, instance, worker)?;
            let owned: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_versions
                     WHERE id=?1 AND worker_id=?2 AND state='validating'
                       AND content_kind='worker' AND substr(main_module,-3)='.py')",
                    params![artifact.version_id, worker],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if !owned {
                return Err(invariant());
            }
            tx.execute(
                "INSERT INTO version_python_prepared
                 (version_id, prepared_identity_sha256, identity_json,
                  artifact_sha256, artifact_size, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    artifact.version_id,
                    artifact.prepared_identity_sha256.as_slice(),
                    artifact.identity_json,
                    artifact.artifact_sha256.as_slice(),
                    i64::try_from(artifact.artifact_size).map_err(|_| invariant())?,
                    artifact.created_at_ms,
                ],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
    }
}

pub(super) fn read_python_prepared_conn(
    connection: &rusqlite::Connection,
    version: VersionId,
) -> Result<Option<PythonPreparedArtifactRecord>, PlatformError> {
    connection
        .query_row(
            "SELECT prepared_identity_sha256, identity_json, artifact_sha256,
                    artifact_size, created_at_ms
             FROM version_python_prepared WHERE version_id=?1",
            [version],
            |row| {
                let identity: Vec<u8> = row.get(0)?;
                let digest: Vec<u8> = row.get(2)?;
                let size: i64 = row.get(3)?;
                Ok(PythonPreparedArtifactRecord {
                    version_id: version,
                    prepared_identity_sha256: identity
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    identity_json: row.get(1)?,
                    artifact_sha256: digest
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    artifact_size: u64::try_from(size)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    created_at_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|_| invariant())
}
