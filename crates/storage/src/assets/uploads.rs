//! Resumable deployment-upload session authority.

use crate::{ControlDb, DeploymentContentKind, DeploymentObjectKind};
use open_compute_core::{
    AccountId, DeploymentId, DeploymentUploadId, ErrorCode, PlatformError, StartupId, WorkerId,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;

/// Durable deployment-upload session state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentUploadStatus {
    /// Objects may still be verified.
    Open,
    /// A fixed deployment identifier is being committed through the ordinary pipeline.
    Finalizing,
    /// The deployment was committed and may be queried after a lost response.
    Committed,
    /// The caller cancelled the session before finalization.
    Aborted,
    /// The unfinished session exceeded its fixed lifetime.
    Expired,
}

impl DeploymentUploadStatus {
    /// Stable current-schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Finalizing => "finalizing",
            Self::Committed => "committed",
            Self::Aborted => "aborted",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "open" => Ok(Self::Open),
            "finalizing" => Ok(Self::Finalizing),
            "committed" => Ok(Self::Committed),
            "aborted" => Ok(Self::Aborted),
            "expired" => Ok(Self::Expired),
            _ => Err(invariant()),
        }
    }
}

/// One declared content-addressed object in an upload session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentUploadObjectRecord {
    /// Object digest.
    pub sha256: [u8; 32],
    /// Semantic inventory kind.
    pub kind: DeploymentObjectKind,
    /// Declared and verified byte length.
    pub size: u64,
    /// Whether the platform verified the actual bytes.
    pub verified: bool,
    /// Verification timestamp.
    pub verified_at_ms: Option<i64>,
}

/// Durable upload-session projection safe for authenticated control responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentUploadRecord {
    /// Session identifier.
    pub id: DeploymentUploadId,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Caller idempotency key.
    pub idempotency_key: String,
    /// Secret-keyed normalized input fingerprint.
    pub input_fingerprint: [u8; 32],
    /// Worker or assets-only content discriminator.
    pub content_kind: DeploymentContentKind,
    /// Optional Worker bundle digest.
    pub bundle_sha256: Option<[u8; 32]>,
    /// Optional Worker bundle length.
    pub bundle_size: Option<u64>,
    /// Canonical asset manifest digest.
    pub manifest_sha256: [u8; 32],
    /// Canonical asset manifest length.
    pub manifest_size: u64,
    /// Canonical manifest bytes.
    pub manifest_json: Vec<u8>,
    /// Canonical asset routing bytes.
    pub routing_config_json: Vec<u8>,
    /// Current state.
    pub status: DeploymentUploadStatus,
    /// Fixed deployment identity once finalization begins.
    pub deployment_id: Option<DeploymentId>,
    /// Secret-keyed fingerprint of the write-only finalize metadata.
    pub finalize_fingerprint: Option<[u8; 32]>,
    /// Exclusive platform startup generation that most recently owned finalization.
    pub finalize_owner_startup_id: Option<StartupId>,
    /// Exact successful finalize response retained for lost-response replay.
    pub finalize_response_json: Option<Vec<u8>>,
    /// Stable terminal pipeline failure retained for lost-response replay.
    pub finalize_error_code: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Expiration timestamp for unfinished work.
    pub expires_at_ms: i64,
    /// Last durable state change.
    pub updated_at_ms: i64,
    /// Canonically ordered inventory.
    pub objects: Vec<DeploymentUploadObjectRecord>,
}

/// Ownership result for one serialized finalize attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentUploadFinalizeDisposition {
    /// This attempt assigned the deployment identity for the first time.
    Reserved,
    /// This attempt reclaimed unfinished work after a prior attempt released its lock.
    Recover,
    /// The exact operation was already committed and has a persisted response.
    Committed,
}

/// Durable upload record plus the action its finalize owner must take.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentUploadFinalize {
    /// Current durable upload authority.
    pub upload: DeploymentUploadRecord,
    /// Whether to create, recover, or replay the fixed deployment.
    pub disposition: DeploymentUploadFinalizeDisposition,
}

/// Durable identity and request proof used to reserve or resume one upload finalization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginDeploymentUploadFinalize {
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Upload session being finalized.
    pub upload_id: DeploymentUploadId,
    /// One fixed deployment identity reused by every retry.
    pub deployment_id: DeploymentId,
    /// HMAC fingerprint of canonical finalization metadata.
    pub finalize_fingerprint: [u8; 32],
    /// Platform startup generation currently recovering the operation.
    pub owner_startup_id: StartupId,
    /// Control-plane wall time.
    pub now_ms: i64,
}

/// One object declared when creating an upload session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDeploymentUploadObject {
    /// Object digest.
    pub sha256: [u8; 32],
    /// Semantic object kind.
    pub kind: DeploymentObjectKind,
    /// Exact byte length.
    pub size: u64,
}

/// Validated upload-session creation input.
#[derive(Clone, Debug)]
pub struct NewDeploymentUpload<'a> {
    /// New session identifier.
    pub id: DeploymentUploadId,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Caller idempotency key.
    pub idempotency_key: &'a str,
    /// Secret-keyed normalized input fingerprint.
    pub input_fingerprint: [u8; 32],
    /// Worker or assets-only content discriminator.
    pub content_kind: DeploymentContentKind,
    /// Optional Worker bundle identity.
    pub bundle: Option<([u8; 32], u64)>,
    /// Canonical asset manifest identity.
    pub manifest_sha256: [u8; 32],
    /// Canonical manifest bytes.
    pub manifest_json: &'a [u8],
    /// Canonical asset routing bytes.
    pub routing_config_json: &'a [u8],
    /// Complete deduplicated inventory, including manifest and optional bundle.
    pub objects: &'a [NewDeploymentUploadObject],
    /// Creation timestamp.
    pub now_ms: i64,
    /// Fixed expiration timestamp.
    pub expires_at_ms: i64,
}

/// Transactional owner for resumable upload state.
#[derive(Clone, Copy, Debug)]
pub struct DeploymentUploadRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> DeploymentUploadRepository<'a> {
    /// Bind the repository to the current control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Create one bounded session or replay the identical idempotent input.
    pub fn create_or_get(
        &self,
        input: &NewDeploymentUpload<'_>,
        max_open_per_worker: u32,
        max_open_per_account: u32,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        validate_new(input, max_open_per_worker, max_open_per_account)?;
        self.db.with_immediate(|tx| {
            expire_open(tx, input.now_ms)?;
            if let Some(existing) =
                read_by_key(tx, input.account_id, input.worker_id, input.idempotency_key)?
            {
                if existing.input_fingerprint != input.input_fingerprint {
                    return Err(conflict());
                }
                return read_tx(tx, existing.id);
            }
            require_live_worker(tx, input.account_id, input.worker_id)?;
            let open: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM deployment_uploads
                     WHERE worker_id = ?1 AND status IN ('open', 'finalizing')",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if open >= i64::from(max_open_per_worker) {
                return Err(PlatformError::new(
                    ErrorCode::AssetLimitExceeded,
                    "Worker deployment-upload session quota was exceeded",
                ));
            }
            let account_open: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM deployment_uploads
                     WHERE account_id = ?1 AND status IN ('open', 'finalizing')",
                    [input.account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if account_open >= i64::from(max_open_per_account) {
                return Err(PlatformError::new(
                    ErrorCode::AssetLimitExceeded,
                    "account deployment-upload session quota was exceeded",
                ));
            }
            tx.execute(
                "INSERT INTO deployment_uploads
                 (id, account_id, worker_id, idempotency_key, input_fingerprint,
                  content_kind, bundle_sha256, bundle_size, manifest_sha256,
                  manifest_size, manifest_json, routing_config_json, status,
                  deployment_id, created_at_ms, expires_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, 'open', NULL, ?13, ?14, ?13)",
                params![
                    input.id.to_string(),
                    input.account_id.to_string(),
                    input.worker_id.to_string(),
                    input.idempotency_key,
                    input.input_fingerprint.as_slice(),
                    input.content_kind.as_str(),
                    input.bundle.as_ref().map(|value| value.0.as_slice()),
                    input
                        .bundle
                        .map(|value| i64::try_from(value.1))
                        .transpose()
                        .map_err(|_| invariant())?,
                    input.manifest_sha256.as_slice(),
                    i64::try_from(input.manifest_json.len()).map_err(|_| invariant())?,
                    input.manifest_json,
                    input.routing_config_json,
                    input.now_ms,
                    input.expires_at_ms,
                ],
            )
            .map_err(|_| db_error())?;
            for object in input.objects {
                tx.execute(
                    "INSERT INTO deployment_upload_objects
                     (session_id, sha256, object_kind, size, verified, verified_at_ms)
                     VALUES (?1, ?2, ?3, ?4, 0, NULL)",
                    params![
                        input.id.to_string(),
                        object.sha256.as_slice(),
                        object.kind.as_str(),
                        i64::try_from(object.size).map_err(|_| invariant())?,
                    ],
                )
                .map_err(|_| db_error())?;
            }
            read_tx(tx, input.id)
        })
    }

    /// Read one account-scoped session and its inventory.
    pub fn get(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        now_ms: i64,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            Ok(record)
        })
    }

    /// Return one declared object before accepting its bytes.
    pub fn object_for_upload(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        sha256: &[u8; 32],
        now_ms: i64,
    ) -> Result<DeploymentUploadObjectRecord, PlatformError> {
        let record = self.get(account_id, worker_id, upload_id, now_ms)?;
        if record.status != DeploymentUploadStatus::Open {
            return Err(conflict());
        }
        record
            .objects
            .into_iter()
            .find(|object| &object.sha256 == sha256)
            .ok_or_else(not_found)
    }

    /// Confirm bytes only after the artifact authority verified digest and length.
    pub fn mark_object_verified(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        sha256: &[u8; 32],
        size: u64,
        now_ms: i64,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.status != DeploymentUploadStatus::Open {
                return Err(conflict());
            }
            let object = record
                .objects
                .iter()
                .find(|object| &object.sha256 == sha256)
                .ok_or_else(not_found)?;
            if object.size != size {
                return Err(conflict());
            }
            tx.execute(
                "UPDATE deployment_upload_objects
                 SET verified = 1, verified_at_ms = COALESCE(verified_at_ms, ?3)
                 WHERE session_id = ?1 AND sha256 = ?2",
                params![upload_id.to_string(), sha256.as_slice(), now_ms],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "UPDATE deployment_uploads SET updated_at_ms = ?2 WHERE id = ?1",
                params![upload_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            read_tx(tx, upload_id)
        })
    }

    /// Persist the one deployment identity used by all finalize retries.
    pub fn begin_finalize(
        &self,
        input: BeginDeploymentUploadFinalize,
    ) -> Result<DeploymentUploadFinalize, PlatformError> {
        let BeginDeploymentUploadFinalize {
            account_id,
            worker_id,
            upload_id,
            deployment_id,
            finalize_fingerprint,
            owner_startup_id,
            now_ms,
        } = input;
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.objects.iter().any(|object| !object.verified) {
                return Err(incomplete());
            }
            let disposition = match record.status {
                DeploymentUploadStatus::Open => {
                    tx.execute(
                        "UPDATE deployment_uploads
                         SET status = 'finalizing', deployment_id = ?2,
                             finalize_fingerprint = ?3, finalize_owner_startup_id = ?4,
                             updated_at_ms = ?5
                         WHERE id = ?1",
                        params![
                            upload_id.to_string(),
                            deployment_id.to_string(),
                            finalize_fingerprint.as_slice(),
                            owner_startup_id.to_string(),
                            now_ms,
                        ],
                    )
                    .map_err(|_| db_error())?;
                    DeploymentUploadFinalizeDisposition::Reserved
                }
                DeploymentUploadStatus::Finalizing
                    if record.deployment_id == Some(deployment_id)
                        && record.finalize_fingerprint.as_ref() == Some(&finalize_fingerprint) =>
                {
                    tx.execute(
                        "UPDATE deployment_uploads
                         SET finalize_owner_startup_id = ?2, updated_at_ms = ?3
                         WHERE id = ?1",
                        params![upload_id.to_string(), owner_startup_id.to_string(), now_ms,],
                    )
                    .map_err(|_| db_error())?;
                    DeploymentUploadFinalizeDisposition::Recover
                }
                DeploymentUploadStatus::Committed
                    if record.deployment_id == Some(deployment_id)
                        && record.finalize_fingerprint.as_ref() == Some(&finalize_fingerprint) =>
                {
                    DeploymentUploadFinalizeDisposition::Committed
                }
                DeploymentUploadStatus::Finalizing
                | DeploymentUploadStatus::Committed
                | DeploymentUploadStatus::Aborted
                | DeploymentUploadStatus::Expired => return Err(conflict()),
            };
            Ok(DeploymentUploadFinalize {
                upload: read_tx(tx, upload_id)?,
                disposition,
            })
        })
    }

    /// Mark a finalized session committed after the ordinary deployment pipeline succeeds.
    pub fn mark_committed(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        deployment_id: DeploymentId,
        response_json: &[u8],
        now_ms: i64,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.deployment_id != Some(deployment_id)
                || !matches!(
                    record.status,
                    DeploymentUploadStatus::Finalizing | DeploymentUploadStatus::Committed
                )
            {
                return Err(conflict());
            }
            tx.execute(
                "UPDATE deployment_uploads
                 SET status = 'committed', finalize_response_json = ?2, updated_at_ms = ?3
                 WHERE id = ?1",
                params![upload_id.to_string(), response_json, now_ms],
            )
            .map_err(|_| db_error())?;
            read_tx(tx, upload_id)
        })
    }

    /// Mark a finalize operation terminal with one stable, secret-safe pipeline error.
    pub fn mark_finalize_failed(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        deployment_id: DeploymentId,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.deployment_id != Some(deployment_id)
                || record.status != DeploymentUploadStatus::Finalizing
            {
                return Err(conflict());
            }
            tx.execute(
                "UPDATE deployment_uploads
                 SET status = 'committed', finalize_error_code = ?2, updated_at_ms = ?3
                 WHERE id = ?1",
                params![upload_id.to_string(), code.as_str(), now_ms],
            )
            .map_err(|_| db_error())?;
            read_tx(tx, upload_id)
        })
    }

    /// Idempotently cancel an open session without deleting shared artifact bytes.
    pub fn abort(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: DeploymentUploadId,
        now_ms: i64,
    ) -> Result<DeploymentUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            match record.status {
                DeploymentUploadStatus::Open => {
                    tx.execute(
                        "UPDATE deployment_uploads
                         SET status = 'aborted', updated_at_ms = ?2 WHERE id = ?1",
                        params![upload_id.to_string(), now_ms],
                    )
                    .map_err(|_| db_error())?;
                }
                DeploymentUploadStatus::Aborted | DeploymentUploadStatus::Expired => {}
                DeploymentUploadStatus::Finalizing | DeploymentUploadStatus::Committed => {
                    return Err(conflict());
                }
            }
            read_tx(tx, upload_id)
        })
    }
}

fn validate_new(
    input: &NewDeploymentUpload<'_>,
    max_open_per_worker: u32,
    max_open_per_account: u32,
) -> Result<(), PlatformError> {
    let bundle_shape = match input.content_kind {
        DeploymentContentKind::Worker => input.bundle.is_some(),
        DeploymentContentKind::AssetsOnly => input.bundle.is_none(),
    };
    if max_open_per_worker == 0
        || max_open_per_account < max_open_per_worker
        || !bundle_shape
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > 128
        || input.manifest_json.is_empty()
        || input.routing_config_json.is_empty()
        || input.expires_at_ms <= input.now_ms
        || input.objects.is_empty()
    {
        return Err(conflict());
    }
    let manifest_size = u64::try_from(input.manifest_json.len()).map_err(|_| conflict())?;
    if !input.objects.iter().any(|object| {
        object.kind == DeploymentObjectKind::AssetManifest
            && object.sha256 == input.manifest_sha256
            && object.size == manifest_size
    }) || input.bundle.is_some_and(|bundle| {
        !input.objects.iter().any(|object| {
            object.kind == DeploymentObjectKind::Bundle
                && object.sha256 == bundle.0
                && object.size == bundle.1
        })
    }) {
        return Err(conflict());
    }
    let mut identities = input
        .objects
        .iter()
        .map(|object| object.sha256)
        .collect::<Vec<_>>();
    identities.sort_unstable();
    if identities.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(conflict());
    }
    Ok(())
}

fn read_by_key(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
    key: &str,
) -> Result<Option<DeploymentUploadRecord>, PlatformError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM deployment_uploads
             WHERE account_id = ?1 AND worker_id = ?2 AND idempotency_key = ?3",
            params![account_id.to_string(), worker_id.to_string(), key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?;
    id.map(|value| {
        let upload_id = DeploymentUploadId::from_str(&value).map_err(|_| invariant())?;
        read_tx(tx, upload_id)
    })
    .transpose()
}

fn read_tx(
    tx: &Transaction<'_>,
    upload_id: DeploymentUploadId,
) -> Result<DeploymentUploadRecord, PlatformError> {
    let mut record = tx
        .query_row(
            "SELECT id, account_id, worker_id, idempotency_key, input_fingerprint,
                    content_kind, bundle_sha256, bundle_size, manifest_sha256,
                    manifest_size, manifest_json, routing_config_json, status,
                    deployment_id, finalize_fingerprint, finalize_owner_startup_id,
                    finalize_response_json, finalize_error_code,
                    created_at_ms, expires_at_ms, updated_at_ms
             FROM deployment_uploads WHERE id = ?1",
            [upload_id.to_string()],
            map_upload,
        )
        .optional()
        .map_err(|_| db_error())?
        .ok_or_else(not_found)?;
    let mut stmt = tx
        .prepare(
            "SELECT sha256, object_kind, size, verified, verified_at_ms
             FROM deployment_upload_objects WHERE session_id = ?1
             ORDER BY object_kind, sha256",
        )
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([upload_id.to_string()], map_object)
        .map_err(|_| db_error())?;
    record.objects = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| db_error())?;
    Ok(record)
}

fn map_upload(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentUploadRecord> {
    let id: String = row.get(0)?;
    let account: String = row.get(1)?;
    let worker: String = row.get(2)?;
    let fingerprint: Vec<u8> = row.get(4)?;
    let kind: String = row.get(5)?;
    let bundle_digest: Option<Vec<u8>> = row.get(6)?;
    let bundle_size: Option<i64> = row.get(7)?;
    let manifest_digest: Vec<u8> = row.get(8)?;
    let manifest_size: i64 = row.get(9)?;
    let status: String = row.get(12)?;
    let deployment: Option<String> = row.get(13)?;
    let finalize_fingerprint: Option<Vec<u8>> = row.get(14)?;
    let finalize_owner: Option<String> = row.get(15)?;
    Ok(DeploymentUploadRecord {
        id: id.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: account.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: worker.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        idempotency_key: row.get(3)?,
        input_fingerprint: fingerprint
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        content_kind: DeploymentContentKind::parse(&kind)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        bundle_sha256: bundle_digest
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        bundle_size: bundle_size
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_sha256: manifest_digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_size: u64::try_from(manifest_size).map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_json: row.get(10)?,
        routing_config_json: row.get(11)?,
        status: DeploymentUploadStatus::parse(&status)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        deployment_id: deployment
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_fingerprint: finalize_fingerprint
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_owner_startup_id: finalize_owner
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_response_json: row.get(16)?,
        finalize_error_code: row.get(17)?,
        created_at_ms: row.get(18)?,
        expires_at_ms: row.get(19)?,
        updated_at_ms: row.get(20)?,
        objects: Vec::new(),
    })
}

fn map_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentUploadObjectRecord> {
    let digest: Vec<u8> = row.get(0)?;
    let kind: String = row.get(1)?;
    let size: i64 = row.get(2)?;
    let verified: i64 = row.get(3)?;
    Ok(DeploymentUploadObjectRecord {
        sha256: digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: parse_object_kind(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        size: u64::try_from(size).map_err(|_| rusqlite::Error::InvalidQuery)?,
        verified: verified == 1,
        verified_at_ms: row.get(4)?,
    })
}

fn parse_object_kind(value: &str) -> Result<DeploymentObjectKind, PlatformError> {
    match value {
        "bundle" => Ok(DeploymentObjectKind::Bundle),
        "asset_manifest" => Ok(DeploymentObjectKind::AssetManifest),
        "asset_blob" => Ok(DeploymentObjectKind::AssetBlob),
        _ => Err(invariant()),
    }
}

fn expire_open(tx: &Transaction<'_>, now_ms: i64) -> Result<(), PlatformError> {
    tx.execute(
        "UPDATE deployment_uploads SET status = 'expired', updated_at_ms = ?1
         WHERE status = 'open' AND expires_at_ms <= ?1",
        [now_ms],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

fn require_live_worker(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<(), PlatformError> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM workers
             WHERE id = ?1 AND account_id = ?2 AND deleted_at_ms IS NULL",
            params![worker_id.to_string(), account_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?;
    found.map(|_| ()).ok_or_else(not_found)
}

fn require_scope(
    record: &DeploymentUploadRecord,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<(), PlatformError> {
    if record.account_id == account_id && record.worker_id == worker_id {
        Ok(())
    } else {
        Err(not_found())
    }
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentNotFound,
        "deployment upload session was not found",
    )
}

fn incomplete() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadIncomplete,
        "deployment upload is missing verified objects",
    )
}

fn conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadConflict,
        "deployment upload conflicts with durable session state",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "deployment upload authority is inconsistent",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "deployment upload database operation failed",
    )
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
