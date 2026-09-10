//! Focused repository lifecycle transitions.

use super::{
    ArtifactRepositoryRecord, ArtifactRepositoryState, CloudflareArtifactsRepository, db_error,
    invariant, read_repository,
};
use open_compute_core::{ArtifactRepoId, PlatformError};
use rusqlite::params;

impl CloudflareArtifactsRepository<'_> {
    /// Complete or fail a reserved fork.
    pub fn finish_repository_fork(
        self,
        id: ArtifactRepoId,
        success: bool,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.finish_repository_state(id, ArtifactRepositoryState::Forking, success, now_ms)
    }

    /// Fail a repository whose filesystem authority is missing or corrupt.
    pub fn fail_repository(
        self,
        id: ArtifactRepoId,
        expected: ArtifactRepositoryState,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        if matches!(
            expected,
            ArtifactRepositoryState::Failed | ArtifactRepositoryState::Tombstoned
        ) {
            return Err(invariant());
        }
        self.finish_repository_state(id, expected, false, now_ms)
    }

    pub(super) fn finish_repository_state(
        self,
        id: ArtifactRepoId,
        expected: ArtifactRepositoryState,
        success: bool,
        now_ms: i64,
    ) -> Result<ArtifactRepositoryRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE artifact_repositories SET state = ?2, updated_at_ms = ?3
                     WHERE id = ?1 AND state = ?4",
                    params![
                        id.to_string(),
                        if success { "ready" } else { "failed" },
                        now_ms,
                        expected.as_str()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(invariant());
            }
            read_repository(tx, id)
        })
    }
}
