use super::*;

impl<'a> VersionUploadRepository<'a> {
    /// Bind the repository to the current control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Create one bounded session or replay the identical idempotent input.
    pub fn create_or_get(
        &self,
        input: &NewVersionUpload<'_>,
        max_open_per_worker: u32,
        max_open_per_account: u32,
    ) -> Result<VersionUploadRecord, PlatformError> {
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
                    "SELECT COUNT(*) FROM version_uploads
                     WHERE worker_id = ?1 AND status IN ('open', 'finalizing')",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if open >= i64::from(max_open_per_worker) {
                return Err(PlatformError::new(
                    ErrorCode::AssetLimitExceeded,
                    "Worker version-upload session quota was exceeded",
                ));
            }
            let account_open: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM version_uploads
                     WHERE account_id = ?1 AND status IN ('open', 'finalizing')",
                    [input.account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if account_open >= i64::from(max_open_per_account) {
                return Err(PlatformError::new(
                    ErrorCode::AssetLimitExceeded,
                    "account version-upload session quota was exceeded",
                ));
            }
            tx.execute(
                "INSERT INTO version_uploads
                 (id, account_id, worker_id, idempotency_key, input_fingerprint,
                  content_kind, bundle_sha256, bundle_size, manifest_sha256,
                  manifest_size, manifest_json, routing_config_json, status,
                  version_id, created_at_ms, expires_at_ms, updated_at_ms)
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
                    "INSERT INTO version_upload_objects
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
        upload_id: VersionUploadId,
        now_ms: i64,
    ) -> Result<VersionUploadRecord, PlatformError> {
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
        upload_id: VersionUploadId,
        sha256: &[u8; 32],
        now_ms: i64,
    ) -> Result<VersionUploadObjectRecord, PlatformError> {
        let record = self.get(account_id, worker_id, upload_id, now_ms)?;
        if record.status != VersionUploadStatus::Open {
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
        upload_id: VersionUploadId,
        sha256: &[u8; 32],
        size: u64,
        now_ms: i64,
    ) -> Result<VersionUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.status != VersionUploadStatus::Open {
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
                "UPDATE version_upload_objects
                 SET verified = 1, verified_at_ms = COALESCE(verified_at_ms, ?3)
                 WHERE session_id = ?1 AND sha256 = ?2",
                params![upload_id.to_string(), sha256.as_slice(), now_ms],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "UPDATE version_uploads SET updated_at_ms = ?2 WHERE id = ?1",
                params![upload_id.to_string(), now_ms],
            )
            .map_err(|_| db_error())?;
            read_tx(tx, upload_id)
        })
    }

    /// Persist the one version identity used by all finalize retries.
    pub fn begin_finalize(
        &self,
        input: BeginVersionUploadFinalize,
    ) -> Result<VersionUploadFinalize, PlatformError> {
        let BeginVersionUploadFinalize {
            account_id,
            worker_id,
            upload_id,
            version_id,
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
                VersionUploadStatus::Open => {
                    tx.execute(
                        "UPDATE version_uploads
                         SET status = 'finalizing', version_id = ?2,
                             finalize_fingerprint = ?3, finalize_owner_startup_id = ?4,
                             updated_at_ms = ?5
                         WHERE id = ?1",
                        params![
                            upload_id.to_string(),
                            version_id.to_string(),
                            finalize_fingerprint.as_slice(),
                            owner_startup_id.to_string(),
                            now_ms,
                        ],
                    )
                    .map_err(|_| db_error())?;
                    VersionUploadFinalizeDisposition::Reserved
                }
                VersionUploadStatus::Finalizing
                    if record.version_id == Some(version_id)
                        && record.finalize_fingerprint.as_ref() == Some(&finalize_fingerprint) =>
                {
                    tx.execute(
                        "UPDATE version_uploads
                         SET finalize_owner_startup_id = ?2, updated_at_ms = ?3
                         WHERE id = ?1",
                        params![upload_id.to_string(), owner_startup_id.to_string(), now_ms,],
                    )
                    .map_err(|_| db_error())?;
                    VersionUploadFinalizeDisposition::Recover
                }
                VersionUploadStatus::Committed
                    if record.version_id == Some(version_id)
                        && record.finalize_fingerprint.as_ref() == Some(&finalize_fingerprint) =>
                {
                    VersionUploadFinalizeDisposition::Committed
                }
                VersionUploadStatus::Finalizing
                | VersionUploadStatus::Committed
                | VersionUploadStatus::Aborted
                | VersionUploadStatus::Expired => return Err(conflict()),
            };
            Ok(VersionUploadFinalize {
                upload: read_tx(tx, upload_id)?,
                disposition,
            })
        })
    }

    /// Mark a finalized session committed after the ordinary version pipeline succeeds.
    pub fn mark_committed(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        upload_id: VersionUploadId,
        version_id: VersionId,
        response_json: &[u8],
        now_ms: i64,
    ) -> Result<VersionUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.version_id != Some(version_id)
                || !matches!(
                    record.status,
                    VersionUploadStatus::Finalizing | VersionUploadStatus::Committed
                )
            {
                return Err(conflict());
            }
            tx.execute(
                "UPDATE version_uploads
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
        upload_id: VersionUploadId,
        version_id: VersionId,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<VersionUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            if record.version_id != Some(version_id)
                || record.status != VersionUploadStatus::Finalizing
            {
                return Err(conflict());
            }
            tx.execute(
                "UPDATE version_uploads
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
        upload_id: VersionUploadId,
        now_ms: i64,
    ) -> Result<VersionUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            expire_open(tx, now_ms)?;
            let record = read_tx(tx, upload_id)?;
            require_scope(&record, account_id, worker_id)?;
            match record.status {
                VersionUploadStatus::Open => {
                    tx.execute(
                        "UPDATE version_uploads
                         SET status = 'aborted', updated_at_ms = ?2 WHERE id = ?1",
                        params![upload_id.to_string(), now_ms],
                    )
                    .map_err(|_| db_error())?;
                }
                VersionUploadStatus::Aborted | VersionUploadStatus::Expired => {}
                VersionUploadStatus::Finalizing | VersionUploadStatus::Committed => {
                    return Err(conflict());
                }
            }
            read_tx(tx, upload_id)
        })
    }
}
