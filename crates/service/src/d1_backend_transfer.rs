//! Durable D1 SQL transfer and time-travel workflows.

use super::{D1BindingService, ensure_d1_storage_headroom, limit_error};
use md5::Md5;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use open_compute_storage::{
    D1_MAX_TRANSFER_SQL_BYTES, D1Engine, D1ExportOptions, D1Paths, D1QueryLimits,
    D1SnapshotRepository, D1TransferAction, D1TransferKind, D1TransferRecord, D1TransferState,
    NewD1Transfer, PlatformStorage,
};
use sha2::{Digest as _, Sha256};
use std::time::Duration;

const D1_TRANSFER_TOKEN_TTL_MS: i64 = 60 * 60 * 1000;

/// Official D1 time-travel source selector after transport validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum D1TimeTravelTarget {
    /// Opaque same-database completed-history bookmark.
    Bookmark(String),
    /// Nearest completed snapshot at or before this Unix millisecond.
    TimestampMs(i64),
}

/// One durable transfer session plus its reconstructable short-lived URL capability.
#[derive(Clone, Debug, PartialEq)]
pub struct D1TransferGrant {
    /// Durable transfer lifecycle record.
    pub transfer: D1TransferRecord,
    /// Opaque capability presented only on the upload/download URL.
    pub token: String,
}

impl D1BindingService {
    /// Generate and persist a restart-safe SQL export anchored to completed history.
    pub async fn begin_export(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        options: D1ExportOptions,
    ) -> Result<D1TransferGrant, PlatformError> {
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        self.coordinator
            .execute(account_id, resource_id, timeout, true, move |context| {
                let now_ms = checked_now_ms()?;
                let expires_at_ms = now_ms
                    .checked_add(D1_TRANSFER_TOKEN_TTL_MS)
                    .ok_or_else(history_invariant)?;
                let repository = D1SnapshotRepository::new(context.storage.db());
                let snapshot = repository
                    .latest_snapshot(account_id, resource_id)?
                    .ok_or_else(history_invariant)?;
                if snapshot.session_version != context.engine.session_version()? {
                    return Err(history_invariant());
                }
                let session_id = uuid::Uuid::now_v7().hyphenated().to_string();
                let filename = format!("export-{resource_id}-{session_id}.sql");
                let token = transfer_token(
                    context.storage,
                    account_id,
                    resource_id,
                    &session_id,
                    D1TransferAction::Download,
                );
                let token_fingerprint = transfer_token_fingerprint(context.storage, &token);
                let transfer = repository.create_transfer(&NewD1Transfer {
                    id: &session_id,
                    account_id,
                    resource_id,
                    kind: D1TransferKind::Export,
                    at_session_version: snapshot.session_version,
                    filename: &filename,
                    etag_md5: None,
                    token_fingerprint: &token_fingerprint,
                    token_action: D1TransferAction::Download,
                    token_expires_at_ms: expires_at_ms,
                    now_ms,
                })?;
                let generation = (|| {
                    let paths = D1Paths::open(context.storage.data_dir().root())?;
                    let catalog =
                        open_compute_storage::D1DatabaseRepository::new(context.storage.db())
                            .get(account_id, resource_id)?;
                    let snapshot_path = paths.resolve_snapshot_key(
                        &snapshot.snapshot_key,
                        account_id,
                        resource_id,
                        snapshot.session_version,
                    )?;
                    let bytes = D1Engine::export_sql(
                        &snapshot_path,
                        &catalog,
                        snapshot.session_version,
                        &options,
                        D1_MAX_TRANSFER_SQL_BYTES,
                    )?;
                    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
                    let size = u64::try_from(bytes.len()).map_err(|_| history_invariant())?;
                    let key = paths.write_transfer(
                        account_id,
                        resource_id,
                        &session_id,
                        &filename,
                        &bytes,
                    )?;
                    repository.complete_export(
                        account_id,
                        &session_id,
                        &key,
                        &sha256,
                        size,
                        checked_now_ms()?,
                    )
                })();
                match generation {
                    Ok(transfer) => Ok(D1TransferGrant { transfer, token }),
                    Err(error) => {
                        if let Ok(failed_at_ms) = checked_now_ms() {
                            let _ = repository.fail_transfer(
                                account_id,
                                &transfer.id,
                                error.code(),
                                failed_at_ms,
                            );
                        }
                        Err(error)
                    }
                }
            })
            .await
    }

    /// Reserve or replay one SQL import upload session for a stable database/ETag input.
    pub async fn begin_import(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        etag_md5: [u8; 16],
    ) -> Result<D1TransferGrant, PlatformError> {
        let timeout = Duration::from_millis(self.config.query_timeout_ms);
        self.coordinator
            .execute(account_id, resource_id, timeout, true, move |context| {
                let repository = D1SnapshotRepository::new(context.storage.db());
                let snapshot = repository
                    .latest_snapshot(account_id, resource_id)?
                    .ok_or_else(history_invariant)?;
                if snapshot.session_version != context.engine.session_version()? {
                    return Err(history_invariant());
                }
                let filename = format!("import-{resource_id}-{}.sql", hex::encode(etag_md5));
                if let Some(existing) = repository.transfer_by_filename(
                    account_id,
                    resource_id,
                    D1TransferKind::Import,
                    &filename,
                )? {
                    if existing.etag_md5 != Some(etag_md5)
                        || matches!(
                            existing.state,
                            D1TransferState::Failed | D1TransferState::Expired
                        )
                    {
                        return Err(idempotency_conflict());
                    }
                    let token = transfer_token(
                        context.storage,
                        account_id,
                        resource_id,
                        &existing.id,
                        D1TransferAction::Upload,
                    );
                    return Ok(D1TransferGrant {
                        transfer: existing,
                        token,
                    });
                }
                let now_ms = checked_now_ms()?;
                let expires_at_ms = now_ms
                    .checked_add(D1_TRANSFER_TOKEN_TTL_MS)
                    .ok_or_else(history_invariant)?;
                let session_id = uuid::Uuid::now_v7().hyphenated().to_string();
                let token = transfer_token(
                    context.storage,
                    account_id,
                    resource_id,
                    &session_id,
                    D1TransferAction::Upload,
                );
                let token_fingerprint = transfer_token_fingerprint(context.storage, &token);
                let transfer = repository.create_transfer(&NewD1Transfer {
                    id: &session_id,
                    account_id,
                    resource_id,
                    kind: D1TransferKind::Import,
                    at_session_version: snapshot.session_version,
                    filename: &filename,
                    etag_md5: Some(&etag_md5),
                    token_fingerprint: &token_fingerprint,
                    token_action: D1TransferAction::Upload,
                    token_expires_at_ms: expires_at_ms,
                    now_ms,
                })?;
                Ok(D1TransferGrant { transfer, token })
            })
            .await
    }

    /// Verify and durably publish one import upload, including quoted-ETag replay evidence.
    pub async fn upload_import(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
        token: String,
        bytes: Vec<u8>,
    ) -> Result<D1TransferRecord, PlatformError> {
        if bytes.is_empty() || bytes.len() > D1_MAX_TRANSFER_SQL_BYTES {
            return Err(limit_error());
        }
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        self.coordinator
            .execute(account_id, resource_id, timeout, true, move |context| {
                let repository = D1SnapshotRepository::new(context.storage.db());
                let token_fingerprint = transfer_token_fingerprint(context.storage, &token);
                let transfer = repository.authorize_transfer_token(
                    account_id,
                    resource_id,
                    &session_id,
                    D1TransferAction::Upload,
                    &token_fingerprint,
                    checked_now_ms()?,
                )?;
                let actual_etag: [u8; 16] = Md5::digest(&bytes).into();
                let expected_etag = transfer.etag_md5.ok_or_else(history_invariant)?;
                if actual_etag != expected_etag {
                    return Err(transfer_integrity_error());
                }
                let sha256: [u8; 32] = Sha256::digest(&bytes).into();
                let size = u64::try_from(bytes.len()).map_err(|_| limit_error())?;
                let paths = D1Paths::open(context.storage.data_dir().root())?;
                let key =
                    D1Paths::transfer_key(account_id, resource_id, &session_id, &transfer.filename);
                if transfer.state == D1TransferState::Uploading {
                    let published = paths.write_transfer(
                        account_id,
                        resource_id,
                        &session_id,
                        &transfer.filename,
                        &bytes,
                    )?;
                    if published != key {
                        return Err(history_invariant());
                    }
                } else {
                    let existing = paths.read_transfer(
                        transfer.file_key.as_deref().ok_or_else(history_invariant)?,
                        account_id,
                        resource_id,
                        &session_id,
                        &transfer.filename,
                    )?;
                    if existing != bytes {
                        return Err(transfer_integrity_error());
                    }
                }
                repository.complete_upload(
                    account_id,
                    &session_id,
                    &key,
                    &expected_etag,
                    &sha256,
                    size,
                    checked_now_ms()?,
                )
            })
            .await
    }

    /// Authenticate one import upload capability before reading its request body.
    pub async fn authorize_import_upload(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
        token: String,
    ) -> Result<D1TransferRecord, PlatformError> {
        self.coordinator
            .execute(
                account_id,
                resource_id,
                Duration::from_millis(self.config.query_timeout_ms),
                false,
                move |context| {
                    let fingerprint = transfer_token_fingerprint(context.storage, &token);
                    D1SnapshotRepository::new(context.storage.db()).authorize_transfer_token(
                        account_id,
                        resource_id,
                        &session_id,
                        D1TransferAction::Upload,
                        &fingerprint,
                        checked_now_ms()?,
                    )
                },
            )
            .await
    }

    /// Apply one verified uploaded SQL import through the shared database lane.
    pub async fn ingest_import(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
    ) -> Result<D1TransferRecord, PlatformError> {
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        let outcome = self
            .coordinator
            .execute(account_id, resource_id, timeout, true, {
                let session_id = session_id.clone();
                move |context| {
                    let repository = D1SnapshotRepository::new(context.storage.db());
                    let transfer = repository.transfer(account_id, &session_id)?;
                    if transfer.resource_id != resource_id {
                        return Err(history_invariant());
                    }
                    if transfer.state == D1TransferState::Complete {
                        return Ok(());
                    }
                    if transfer.state != D1TransferState::Uploaded
                        || transfer.at_session_version != context.engine.session_version()?
                    {
                        return Err(history_invariant());
                    }
                    context.mark_mutation();
                    ensure_d1_storage_headroom(context.storage)?;
                    let paths = D1Paths::open(context.storage.data_dir().root())?;
                    let bytes = paths.read_transfer(
                        transfer.file_key.as_deref().ok_or_else(history_invariant)?,
                        account_id,
                        resource_id,
                        &session_id,
                        &transfer.filename,
                    )?;
                    verify_import_bytes(&transfer, &bytes)?;
                    let sql =
                        std::str::from_utf8(&bytes).map_err(|_| transfer_integrity_error())?;
                    context.engine.import_sql(
                        sql,
                        D1QueryLimits::batch(context.config)?,
                        |result| {
                            repository
                                .begin_ingest(
                                    account_id,
                                    &session_id,
                                    result.num_queries,
                                    result.duration_ms,
                                    result.rows_read,
                                    result.rows_written,
                                    result.size_after,
                                    checked_now_ms()?,
                                )
                                .map(|_| ())
                        },
                    )?;
                    Ok(())
                }
            })
            .await;
        if let Err(error) = &outcome
            && error.code() != ErrorCode::D1ResultUnknown
            && let Ok(failed_at_ms) = checked_now_ms()
        {
            let _ = D1SnapshotRepository::new(self.storage.db()).fail_transfer(
                account_id,
                &session_id,
                error.code(),
                failed_at_ms,
            );
        }
        outcome?;
        D1SnapshotRepository::new(self.storage.db()).transfer(account_id, &session_id)
    }

    /// Poll one transfer after running crash reconciliation through the database lane.
    pub async fn transfer(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
    ) -> Result<D1TransferRecord, PlatformError> {
        self.coordinator
            .execute(
                account_id,
                resource_id,
                Duration::from_millis(self.config.query_timeout_ms),
                false,
                move |context| {
                    D1SnapshotRepository::new(context.storage.db())
                        .transfer(account_id, &session_id)
                },
            )
            .await
    }

    /// Read one export session and reconstruct only its scoped download capability.
    pub async fn export_transfer(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
    ) -> Result<D1TransferGrant, PlatformError> {
        let transfer = self
            .transfer(account_id, resource_id, session_id.clone())
            .await?;
        if transfer.kind != D1TransferKind::Export {
            return Err(history_invariant());
        }
        Ok(D1TransferGrant {
            transfer,
            token: transfer_token(
                &self.storage,
                account_id,
                resource_id,
                &session_id,
                D1TransferAction::Download,
            ),
        })
    }

    /// Authorize and read one completed SQL export body for the download endpoint.
    pub async fn download_export(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: String,
        token: String,
    ) -> Result<Vec<u8>, PlatformError> {
        self.coordinator
            .execute(
                account_id,
                resource_id,
                Duration::from_millis(self.config.query_timeout_ms),
                false,
                move |context| {
                    let repository = D1SnapshotRepository::new(context.storage.db());
                    let token_fingerprint = transfer_token_fingerprint(context.storage, &token);
                    let transfer = repository.authorize_transfer_token(
                        account_id,
                        resource_id,
                        &session_id,
                        D1TransferAction::Download,
                        &token_fingerprint,
                        checked_now_ms()?,
                    )?;
                    let paths = D1Paths::open(context.storage.data_dir().root())?;
                    let bytes = paths.read_transfer(
                        transfer.file_key.as_deref().ok_or_else(history_invariant)?,
                        account_id,
                        resource_id,
                        &session_id,
                        &transfer.filename,
                    )?;
                    verify_export_bytes(&transfer, &bytes)?;
                    Ok(bytes)
                },
            )
            .await
    }

    /// Read the tenant `user_version` through the serialized database lane.
    pub async fn user_version(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<u32, PlatformError> {
        self.run_control(account_id, resource_id, false, |engine, _| {
            engine.user_version()
        })
        .await
    }

    /// Resolve one completed history point to an opaque same-database bookmark.
    pub async fn time_travel_bookmark(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        timestamp_ms: Option<i64>,
    ) -> Result<String, PlatformError> {
        let version = self
            .coordinator
            .execute(
                account_id,
                resource_id,
                Duration::from_millis(self.config.query_timeout_ms),
                false,
                move |context| {
                    let repository = D1SnapshotRepository::new(context.storage.db());
                    let snapshot = match timestamp_ms {
                        Some(timestamp) => {
                            repository.snapshot_at_or_before(account_id, resource_id, timestamp)?
                        }
                        None => repository.latest_snapshot(account_id, resource_id)?,
                    }
                    .ok_or_else(|| {
                        PlatformError::new(
                            ErrorCode::ResourceNotFound,
                            "D1 completed history point was not found",
                        )
                    })?;
                    Ok(snapshot.session_version)
                },
            )
            .await?;
        self.storage
            .crypto()
            .seal_d1_bookmark(account_id, resource_id, version)
    }

    /// Seal a bookmark for one exact completed history version.
    pub async fn bookmark_at_version(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        version: u64,
    ) -> Result<String, PlatformError> {
        self.coordinator
            .execute(
                account_id,
                resource_id,
                Duration::from_millis(self.config.query_timeout_ms),
                false,
                move |context| {
                    D1SnapshotRepository::new(context.storage.db())
                        .snapshot(account_id, resource_id, version)
                        .map(|_| ())
                },
            )
            .await?;
        self.storage
            .crypto()
            .seal_d1_bookmark(account_id, resource_id, version)
    }

    /// Restore this database identity to a completed bookmark or timestamp snapshot.
    pub async fn time_travel_restore(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        target: D1TimeTravelTarget,
    ) -> Result<String, PlatformError> {
        let timeout = Duration::from_millis(self.config.batch_timeout_ms);
        let result_version = self
            .coordinator
            .execute(account_id, resource_id, timeout, true, move |context| {
                context.mark_mutation();
                ensure_d1_storage_headroom(context.storage)?;
                let repository = D1SnapshotRepository::new(context.storage.db());
                let latest = repository
                    .latest_snapshot(account_id, resource_id)?
                    .ok_or_else(history_invariant)?;
                let source = match target {
                    D1TimeTravelTarget::Bookmark(bookmark) => {
                        let version = context.storage.crypto().open_d1_bookmark(
                            account_id,
                            resource_id,
                            &bookmark,
                        )?;
                        repository.snapshot(account_id, resource_id, version)?
                    }
                    D1TimeTravelTarget::TimestampMs(timestamp) => repository
                        .snapshot_at_or_before(account_id, resource_id, timestamp)?
                        .ok_or_else(|| {
                            PlatformError::new(
                                ErrorCode::ResourceNotFound,
                                "D1 completed history point was not found",
                            )
                        })?,
                };
                if source.session_version > latest.session_version
                    || latest.session_version != context.engine.session_version()?
                {
                    return Err(history_invariant());
                }
                let canonical = format!(
                    "d1-time-travel-restore\0{account_id}\0{resource_id}\0{}\0{}",
                    source.session_version, latest.session_version
                );
                let fingerprint = context
                    .storage
                    .crypto()
                    .fingerprint_request(canonical.as_bytes());
                let intent_id = uuid::Uuid::now_v7().hyphenated().to_string();
                let intent = repository.prepare_restore(
                    account_id,
                    resource_id,
                    &intent_id,
                    source.session_version,
                    latest.session_version,
                    &fingerprint,
                    checked_now_ms()?,
                )?;
                let paths = D1Paths::open(context.storage.data_dir().root())?;
                let source_path = paths.resolve_snapshot_key(
                    &source.snapshot_key,
                    account_id,
                    resource_id,
                    source.session_version,
                )?;
                let catalog = open_compute_storage::D1DatabaseRepository::new(context.storage.db())
                    .get(account_id, resource_id)?;
                context
                    .engine
                    .restore_in_place(
                        &source_path,
                        &catalog,
                        source.session_version,
                        intent.result_session_version,
                    )
                    .map_err(|_| result_unknown_error())?;
                Ok(intent.result_session_version)
            })
            .await?;
        self.storage
            .crypto()
            .seal_d1_bookmark(account_id, resource_id, result_version)
    }
}

fn checked_now_ms() -> Result<i64, PlatformError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| history_invariant())?;
    i64::try_from(duration.as_millis()).map_err(|_| history_invariant())
}

fn history_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "D1 completed history does not match the live database",
    )
}

fn result_unknown_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1ResultUnknown,
        "D1 restore publication result is unknown",
    )
}

fn transfer_token(
    storage: &PlatformStorage,
    account_id: AccountId,
    resource_id: ResourceId,
    session_id: &str,
    action: D1TransferAction,
) -> String {
    let canonical = format!(
        "d1-transfer-capability-v1\0{account_id}\0{resource_id}\0{session_id}\0{}",
        action.as_str()
    );
    hex::encode(storage.crypto().fingerprint_request(canonical.as_bytes()))
}

fn transfer_token_fingerprint(storage: &PlatformStorage, token: &str) -> [u8; 32] {
    let mut canonical = b"d1-transfer-presented-token-v1\0".to_vec();
    canonical.extend_from_slice(token.as_bytes());
    storage.crypto().fingerprint_request(&canonical)
}

fn verify_import_bytes(transfer: &D1TransferRecord, bytes: &[u8]) -> Result<(), PlatformError> {
    let md5: [u8; 16] = Md5::digest(bytes).into();
    verify_export_bytes(transfer, bytes)?;
    if transfer.etag_md5 != Some(md5) {
        return Err(transfer_integrity_error());
    }
    Ok(())
}

fn verify_export_bytes(transfer: &D1TransferRecord, bytes: &[u8]) -> Result<(), PlatformError> {
    let size = u64::try_from(bytes.len()).map_err(|_| transfer_integrity_error())?;
    let sha256: [u8; 32] = Sha256::digest(bytes).into();
    if transfer.size_bytes != Some(size) || transfer.sha256 != Some(sha256) {
        return Err(transfer_integrity_error());
    }
    Ok(())
}

fn transfer_integrity_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ArtifactIntegrityError,
        "D1 transfer body failed integrity verification",
    )
}

fn idempotency_conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::IdempotencyConflict,
        "D1 transfer replay conflicts with terminal authority",
    )
}
