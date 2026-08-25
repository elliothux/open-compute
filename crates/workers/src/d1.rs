//! P0.6 D1 database lifecycle driver.

use crate::{ReconcileOutcome, ResourceDriver, ResourceHealth};
use open_compute_core::{
    BindingKind, ErrorCode, PlatformError, ResourceAvailability, ResourceState,
};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, D1DatabaseRepository, D1Engine, D1Paths, PlatformStorage,
    ResourceRecord,
};

/// Static filesystem and SQLite driver for `d1_database` resources.
#[derive(Debug)]
pub struct D1ResourceDriver<'a> {
    storage: &'a PlatformStorage,
    quota_bytes: u64,
}

impl<'a> D1ResourceDriver<'a> {
    /// Bind platform authority and the frozen quota used by new databases.
    #[must_use]
    pub const fn new(storage: &'a PlatformStorage, quota_bytes: u64) -> Self {
        Self {
            storage,
            quota_bytes,
        }
    }

    fn paths(&self) -> Result<D1Paths, PlatformError> {
        D1Paths::open(self.storage.data_dir().root())
    }

    fn catalog(
        &self,
        resource: &ResourceRecord,
    ) -> Result<open_compute_storage::D1DatabaseRecord, PlatformError> {
        D1DatabaseRepository::new(self.storage.db()).get(resource.account_id, resource.id)
    }

    fn verify_live(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        let record = self.catalog(resource)?;
        let path =
            paths.resolve_storage_key(&record.storage_key, resource.account_id, resource.id)?;
        let engine = D1Engine::from_record(path, &record)?;
        engine.quick_check()
    }
}

impl ResourceDriver for D1ResourceDriver<'_> {
    fn kind(&self) -> BindingKind {
        BindingKind::D1Database
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if resource.state != ResourceState::Creating
            || resource.kind != BindingKind::D1Database
            || self.quota_bytes < 64 * 1024 * 1024
        {
            return Err(invariant());
        }
        let paths = self.paths()?;
        let storage_key = D1Paths::storage_key(resource.account_id, resource.id);
        let catalog = D1DatabaseRepository::new(self.storage.db());
        let record = match catalog.get(resource.account_id, resource.id) {
            Ok(record) => record,
            Err(error) if error.code() == ErrorCode::ResourceNotFound => catalog.ensure_database(
                resource,
                &storage_key,
                D1_DATABASE_SCHEMA_VERSION,
                self.quota_bytes,
            )?,
            Err(error) => return Err(error),
        };
        if record.storage_key != storage_key {
            return Err(invariant());
        }
        if record.restore_backup_id.is_some() {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "D1 restore must resume through its product controller",
            ));
        }
        let live = paths.resolve_storage_key(&storage_key, resource.account_id, resource.id)?;
        if live.exists() {
            return D1Engine::from_record(live, &record)?.quick_check();
        }
        let candidates = paths.staging_candidates(resource.id)?;
        if candidates.len() > 1 {
            return Err(invariant());
        }
        if let Some(staging) = candidates.first() {
            let staged = staging.join("data.sqlite");
            if D1Engine::from_record(staged, &record)
                .and_then(|engine| engine.quick_check())
                .is_ok()
            {
                return paths.publish_staging(staging, resource.account_id, resource.id);
            }
            paths.remove_operation_dir(staging)?;
        }
        let staging = paths.create_database_staging(resource.id)?;
        let result = (|| {
            D1Engine::create(
                &staging.join("data.sqlite"),
                resource.account_id,
                resource.id,
                resource.created_at_ms,
                self.quota_bytes,
            )?;
            paths.publish_staging(&staging, resource.account_id, resource.id)?;
            self.verify_live(resource)
        })();
        if result.is_err() && staging.exists() {
            let _ = paths.remove_operation_dir(&staging);
        }
        result
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        let paths = self.paths()?;
        match resource.state {
            ResourceState::Creating => {
                let record = match self.catalog(resource) {
                    Ok(record) => record,
                    Err(error) if error.code() == ErrorCode::ResourceNotFound => {
                        return Ok(ReconcileOutcome::Absent);
                    }
                    Err(error) => return Err(error),
                };
                let live = paths.resolve_storage_key(
                    &record.storage_key,
                    resource.account_id,
                    resource.id,
                )?;
                if live.exists() {
                    D1Engine::from_record(live, &record)?.quick_check()?;
                    return Ok(ReconcileOutcome::Ready);
                }
                let candidates = paths.staging_candidates(resource.id)?;
                if candidates.len() > 1 {
                    return Err(invariant());
                }
                let Some(staging) = candidates.first() else {
                    return Ok(if record.restore_backup_id.is_some() {
                        ReconcileOutcome::Deferred
                    } else {
                        ReconcileOutcome::Absent
                    });
                };
                let staged = staging.join("data.sqlite");
                if D1Engine::from_record(staged, &record)
                    .and_then(|engine| engine.quick_check())
                    .is_err()
                {
                    paths.remove_operation_dir(staging)?;
                    return Ok(if record.restore_backup_id.is_some() {
                        ReconcileOutcome::Deferred
                    } else {
                        ReconcileOutcome::Absent
                    });
                }
                paths.publish_staging(staging, resource.account_id, resource.id)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Ready => {
                self.verify_live(resource)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Deleting => Ok(
                if paths
                    .database_dir(resource.account_id, resource.id)
                    .exists()
                {
                    ReconcileOutcome::Ready
                } else {
                    ReconcileOutcome::Deleted
                },
            ),
            ResourceState::Tombstoned => Ok(ReconcileOutcome::Deleted),
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        let live = paths.database_dir(resource.account_id, resource.id);
        if !live.exists() {
            return Ok(());
        }
        let record = self.catalog(resource)?;
        let engine = D1Engine::from_record(live.join("data.sqlite"), &record)?;
        engine.checkpoint(true)?;
        paths.quarantine(resource.account_id, resource.id)?;
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        if paths
            .database_dir(resource.account_id, resource.id)
            .exists()
        {
            return Err(invariant());
        }
        for quarantine in paths.quarantine_candidates(resource.id)? {
            paths.remove_operation_dir(&quarantine)?;
        }
        Ok(())
    }

    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        match self.verify_live(resource) {
            Ok(()) => Ok(ResourceHealth::healthy()),
            Err(error) if error.code() == ErrorCode::D1DatabaseCorrupt => Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("D1_DATABASE_CORRUPT"),
            }),
            Err(error) if error.code() == ErrorCode::D1IdentityMismatch => Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("D1_IDENTITY_MISMATCH"),
            }),
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::ResourceUnavailable
                        | ErrorCode::D1Overloaded
                        | ErrorCode::PathInvalid
                ) =>
            {
                Ok(ResourceHealth {
                    availability: ResourceAvailability::Unavailable,
                    code: Some("D1_UNAVAILABLE"),
                })
            }
            Err(error) => Err(error),
        }
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "D1 lifecycle reconciliation invariant failed",
    )
}

#[cfg(test)]
#[path = "d1_tests.rs"]
mod tests;
