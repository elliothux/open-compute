use super::*;

impl<'a> R2MultipartRepository<'a> {
    /// Bind the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Reserve a tenant upload id before the provider create returns.
    pub fn insert_initiating(
        &self,
        record: &R2MultipartUploadRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if record.state != R2MultipartState::Initiating || record.provider_upload_id.is_some() {
            return Err(invariant());
        }
        self.insert_row(record, now_ms)
    }

    /// Persist the provider id while still initiating so restart can abort it.
    pub fn record_provider_id(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        provider_upload_id: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET provider_upload_id = ?1, updated_at_ms = ?2
                     WHERE upload_id = ?3 AND account_id = ?4 AND resource_id = ?5
                       AND state = 'initiating' AND provider_upload_id IS NULL",
                    params![
                        provider_upload_id,
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Admit an initiating upload as open after the provider id is durable.
    pub fn promote_open(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'open', updated_at_ms = ?1
                     WHERE upload_id = ?2 AND account_id = ?3 AND resource_id = ?4
                       AND state = 'initiating' AND provider_upload_id IS NOT NULL",
                    params![
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Record that provider create may have succeeded without an observable response.
    pub fn mark_create_unknown(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'create_unknown', updated_at_ms = ?1
                     WHERE upload_id = ?2 AND account_id = ?3 AND resource_id = ?4
                       AND state = 'initiating' AND provider_upload_id IS NULL",
                    params![
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Convert startup-left initiating rows into explicit unknown outcomes.
    pub fn mark_resource_initiating_unknown(
        &self,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'create_unknown', updated_at_ms = ?1
                     WHERE resource_id = ?2 AND state = 'initiating'
                       AND provider_upload_id IS NULL",
                    params![now_ms, resource_id.to_string()],
                )
                .map_err(|_| db_error())?;
            u64::try_from(changed).map_err(|_| invariant())
        })
    }

    /// Delete a failed initiating row so the tenant never observes it.
    pub fn delete_initiating(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
    ) -> Result<Option<R2MultipartUploadRecord>, PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_upload(tx, account_id, resource_id, upload_id)?;
            let Some(record) = record else {
                return Ok(None);
            };
            if record.state != R2MultipartState::Initiating {
                return Err(multipart_invalid());
            }
            tx.execute(
                "DELETE FROM r2_multipart_uploads
                 WHERE upload_id = ?1 AND account_id = ?2 AND resource_id = ?3 AND state = 'initiating'",
                params![upload_id, account_id.to_string(), resource_id.to_string()],
            )
            .map_err(|_| db_error())?;
            Ok(Some(record))
        })
    }

    /// Load one account-scoped upload.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
    ) -> Result<Option<R2MultipartUploadRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_upload(conn, account_id, resource_id, upload_id))
    }

    /// Transition `open` to `completing` for a matching key.
    pub fn begin_complete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        completion_manifest: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'completing', completion_manifest = ?1, updated_at_ms = ?2
                     WHERE upload_id = ?3 AND account_id = ?4 AND resource_id = ?5
                       AND object_key = ?6 AND state = 'open'
                       AND completion_manifest IS NULL AND completed_metadata IS NULL",
                    params![
                        completion_manifest,
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                        object_key,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Mark a completing upload as completed.
    pub fn finish_complete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        completed_metadata: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'completed', completed_metadata = ?1, updated_at_ms = ?2
                     WHERE upload_id = ?3 AND account_id = ?4 AND resource_id = ?5
                       AND object_key = ?6 AND state = 'completing'
                       AND completion_manifest IS NOT NULL AND completed_metadata IS NULL",
                    params![
                        completed_metadata,
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                        object_key,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Return a completing upload to `open` after a known complete failure.
    pub fn revert_complete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'open', completion_manifest = NULL, updated_at_ms = ?1
                     WHERE upload_id = ?2 AND account_id = ?3 AND resource_id = ?4
                       AND object_key = ?5 AND state = 'completing'
                       AND completed_metadata IS NULL",
                    params![
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                        object_key,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Transition `open` to `aborting`. Completing/completed rows fail closed.
    pub fn begin_abort(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.transition(
            account_id,
            resource_id,
            upload_id,
            object_key,
            (R2MultipartState::Open, R2MultipartState::Aborting),
            now_ms,
        )
    }

    /// Mark an aborting upload as aborted.
    pub fn finish_abort(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.transition(
            account_id,
            resource_id,
            upload_id,
            object_key,
            (R2MultipartState::Aborting, R2MultipartState::Aborted),
            now_ms,
        )
    }

    /// Insert or replace one uploaded part on an open upload.
    pub fn upsert_part(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        part: &R2MultipartPartRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let record = read_upload(tx, account_id, resource_id, upload_id)?
                .ok_or_else(multipart_invalid)?;
            if record.state != R2MultipartState::Open || record.object_key != object_key {
                return Err(multipart_invalid());
            }
            tx.execute(
                "INSERT INTO r2_multipart_parts
                 (upload_id, part_number, etag, size, uploaded_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(upload_id, part_number) DO UPDATE SET
                   etag = excluded.etag,
                   size = excluded.size,
                   uploaded_at_ms = excluded.uploaded_at_ms",
                params![
                    upload_id,
                    i64::from(part.part_number),
                    part.etag,
                    i64::try_from(part.size).map_err(|_| invariant())?,
                    now_ms
                ],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Load stored parts ordered by part number.
    pub fn list_parts(&self, upload_id: &str) -> Result<Vec<R2MultipartPartRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT part_number, etag, size FROM r2_multipart_parts
                     WHERE upload_id = ?1 ORDER BY part_number",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([upload_id], |row| {
                    Ok(R2MultipartPartRecord {
                        part_number: i32::try_from(row.get::<_, i64>(0)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        etag: row.get(1)?,
                        size: u64::try_from(row.get::<_, i64>(2)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    })
                })
                .map_err(|_| db_error())?;
            let mut parts = Vec::new();
            for row in rows {
                parts.push(row.map_err(|_| invariant())?);
            }
            Ok(parts)
        })
    }

    /// Load every multipart row owned by one logical bucket.
    pub fn list_for_resource(
        &self,
        resource_id: ResourceId,
    ) -> Result<Vec<R2MultipartUploadRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT upload_id, resource_id, account_id, object_key, provider_upload_id,
                            storage_class, http_metadata, custom_metadata, ssec_key_md5,
                            ssec_envelope, object_version, completion_manifest,
                            completed_metadata, state
                     FROM r2_multipart_uploads
                     WHERE resource_id = ?1 ORDER BY created_at_ms, upload_id",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([resource_id.to_string()], map_upload)
                .map_err(|_| db_error())?;
            let mut uploads = Vec::new();
            for row in rows {
                uploads.push(row.map_err(|_| invariant())?);
            }
            Ok(uploads)
        })
    }

    /// Claim an unknown provider upload for cleanup without exposing it to the tenant.
    pub fn claim_unknown_for_abort(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        provider_upload_id: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET provider_upload_id = ?1, state = 'aborting', updated_at_ms = ?2
                     WHERE upload_id = ?3 AND account_id = ?4 AND resource_id = ?5
                       AND state = 'create_unknown' AND provider_upload_id IS NULL",
                    params![
                        provider_upload_id,
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    /// Remove one unknown create after an authoritative provider listing proves no upload exists.
    pub fn delete_create_unknown(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM r2_multipart_uploads
                     WHERE upload_id = ?1 AND account_id = ?2 AND resource_id = ?3
                       AND state = 'create_unknown' AND provider_upload_id IS NULL",
                    params![upload_id, account_id.to_string(), resource_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            Ok(())
        })
    }

    /// Move any provider-backed, nonterminal upload into deletion cleanup.
    ///
    /// `initiating` is accepted only for a caller that has proved the foreground create can no
    /// longer publish a tenant response (startup recovery or failed local admission).
    pub fn claim_for_cleanup(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = 'aborting', completion_manifest = NULL, updated_at_ms = ?1
                     WHERE upload_id = ?2 AND account_id = ?3 AND resource_id = ?4
                       AND state IN ('initiating', 'open', 'completing')
                       AND provider_upload_id IS NOT NULL",
                    params![
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }

    fn insert_row(
        self,
        record: &R2MultipartUploadRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if record.ssec_key_md5.is_some() != record.ssec_envelope.is_some()
            || !valid_ssec_key_md5(record.ssec_key_md5.as_deref())
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO r2_multipart_uploads
                 (upload_id, resource_id, account_id, object_key, provider_upload_id,
                  storage_class, http_metadata, custom_metadata, ssec_key_md5, ssec_envelope,
                  object_version, completion_manifest, completed_metadata, state,
                  created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
                params![
                    record.upload_id,
                    record.resource_id.to_string(),
                    record.account_id.to_string(),
                    record.object_key,
                    record.provider_upload_id,
                    record.storage_class,
                    record.http_metadata,
                    record.custom_metadata,
                    record.ssec_key_md5,
                    record.ssec_envelope,
                    record.object_version,
                    record.completion_manifest,
                    record.completed_metadata,
                    record.state.as_str(),
                    now_ms,
                ],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
    }

    fn transition(
        self,
        account_id: AccountId,
        resource_id: ResourceId,
        upload_id: &str,
        object_key: &str,
        states: (R2MultipartState, R2MultipartState),
        now_ms: i64,
    ) -> Result<R2MultipartUploadRecord, PlatformError> {
        let (from, to) = states;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_multipart_uploads
                     SET state = ?1, updated_at_ms = ?2
                     WHERE upload_id = ?3 AND account_id = ?4 AND resource_id = ?5
                       AND object_key = ?6 AND state = ?7",
                    params![
                        to.as_str(),
                        now_ms,
                        upload_id,
                        account_id.to_string(),
                        resource_id.to_string(),
                        object_key,
                        from.as_str(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(multipart_invalid());
            }
            read_upload(tx, account_id, resource_id, upload_id)?.ok_or_else(invariant)
        })
    }
}
