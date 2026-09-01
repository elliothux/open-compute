//! Account-scoped durable R2 multipart upload authority.

use crate::ControlDb;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::{OptionalExtension, params};
use std::fmt;
use std::str::FromStr as _;

/// Lifecycle of one tenant-visible multipart upload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum R2MultipartState {
    /// Catalog reserved; provider create may still be in flight.
    Initiating,
    /// Provider create may have succeeded but its response was not observed.
    CreateUnknown,
    /// Parts may still be uploaded.
    Open,
    /// Complete is in flight or awaiting reconciliation.
    Completing,
    /// Object has been committed.
    Completed,
    /// Abort is in flight or awaiting reconciliation.
    Aborting,
    /// Upload was aborted.
    Aborted,
}

impl R2MultipartState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Initiating => "initiating",
            Self::CreateUnknown => "create_unknown",
            Self::Open => "open",
            Self::Completing => "completing",
            Self::Completed => "completed",
            Self::Aborting => "aborting",
            Self::Aborted => "aborted",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "initiating" => Ok(Self::Initiating),
            "create_unknown" => Ok(Self::CreateUnknown),
            "open" => Ok(Self::Open),
            "completing" => Ok(Self::Completing),
            "completed" => Ok(Self::Completed),
            "aborting" => Ok(Self::Aborting),
            "aborted" => Ok(Self::Aborted),
            _ => Err(invariant()),
        }
    }
}

/// Durable multipart upload mapping. SSE-C plaintext is never stored.
#[derive(Clone, Eq, PartialEq)]
pub struct R2MultipartUploadRecord {
    /// Tenant-visible upload id.
    pub upload_id: String,
    /// Owning logical bucket.
    pub resource_id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Exact object key.
    pub object_key: String,
    /// Provider multipart id, absent only while initiating.
    pub provider_upload_id: Option<String>,
    /// Worker API storage class token.
    pub storage_class: String,
    /// Canonical HTTP metadata JSON.
    pub http_metadata: String,
    /// Canonical custom metadata JSON.
    pub custom_metadata: String,
    /// Public `ssecKeyMd5` when the upload is SSE-C.
    pub ssec_key_md5: Option<String>,
    /// AEAD envelope JSON for the SSE-C key. Never plaintext.
    pub ssec_envelope: Option<String>,
    /// Object version allocated at create.
    pub object_version: String,
    /// Canonical exact ordered completion request, once completion starts.
    pub completion_manifest: Option<String>,
    /// Canonical completed object metadata, once completion commits.
    pub completed_metadata: Option<String>,
    /// Current lifecycle state.
    pub state: R2MultipartState,
}

impl fmt::Debug for R2MultipartUploadRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("R2MultipartUploadRecord")
            .field("upload_id", &self.upload_id)
            .field("resource_id", &self.resource_id)
            .field("account_id", &self.account_id)
            .field("object_key", &self.object_key)
            .field("provider_upload_id", &self.provider_upload_id)
            .field("storage_class", &self.storage_class)
            .field("ssec_key_md5", &self.ssec_key_md5)
            .field(
                "ssec_envelope",
                &self.ssec_envelope.as_ref().map(|_| "present"),
            )
            .field("object_version", &self.object_version)
            .field(
                "completion_manifest",
                &self.completion_manifest.as_ref().map(|_| "present"),
            )
            .field(
                "completed_metadata",
                &self.completed_metadata.as_ref().map(|_| "present"),
            )
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// One stored multipart part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2MultipartPartRecord {
    /// Part number in `1..=10000`.
    pub part_number: i32,
    /// Provider part `ETag`.
    pub etag: String,
    /// Part size in bytes.
    pub size: u64,
}

/// Typed repository for multipart upload rows.
#[derive(Clone, Copy, Debug)]
pub struct R2MultipartRepository<'a> {
    db: &'a ControlDb,
}

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
        if record.ssec_key_md5.is_some() != record.ssec_envelope.is_some() {
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

fn map_upload(row: &rusqlite::Row<'_>) -> rusqlite::Result<R2MultipartUploadRecord> {
    let record = R2MultipartUploadRecord {
        upload_id: row.get(0)?,
        resource_id: ResourceId::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&row.get::<_, String>(2)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(3)?,
        provider_upload_id: row.get(4)?,
        storage_class: row.get(5)?,
        http_metadata: row.get(6)?,
        custom_metadata: row.get(7)?,
        ssec_key_md5: row.get(8)?,
        ssec_envelope: row.get(9)?,
        object_version: row.get(10)?,
        completion_manifest: row.get(11)?,
        completed_metadata: row.get(12)?,
        state: R2MultipartState::parse(&row.get::<_, String>(13)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    if !valid_upload_record(&record) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(record)
}

fn valid_upload_record(record: &R2MultipartUploadRecord) -> bool {
    if record.ssec_key_md5.is_some() != record.ssec_envelope.is_some()
        || (record.state != R2MultipartState::Initiating
            && record.state != R2MultipartState::CreateUnknown
            && record.provider_upload_id.is_none())
    {
        return false;
    }
    let manifest_expected = matches!(
        record.state,
        R2MultipartState::Completing | R2MultipartState::Completed
    );
    if manifest_expected != record.completion_manifest.is_some()
        || (record.state == R2MultipartState::Completed) != record.completed_metadata.is_some()
    {
        return false;
    }
    if let Some(raw) = record.completion_manifest.as_deref() {
        let Ok(parts) = serde_json::from_str::<Vec<StoredCompletionPart>>(raw) else {
            return false;
        };
        if parts.is_empty() || serde_json::to_string(&parts).ok().as_deref() != Some(raw) {
            return false;
        }
        let mut previous = 0_i64;
        for part in parts {
            if !(1..=10_000).contains(&part.part_number)
                || part.part_number <= previous
                || part.etag.is_empty()
            {
                return false;
            }
            previous = part.part_number;
        }
    }
    if let Some(raw) = record.completed_metadata.as_deref() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return false;
        };
        if value.get("key").and_then(serde_json::Value::as_str) != Some(record.object_key.as_str())
            || value.get("version").and_then(serde_json::Value::as_str)
                != Some(record.object_version.as_str())
        {
            return false;
        }
    }
    true
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCompletionPart {
    part_number: i64,
    etag: String,
}

fn read_upload(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
    upload_id: &str,
) -> Result<Option<R2MultipartUploadRecord>, PlatformError> {
    match conn
        .query_row(
            "SELECT upload_id, resource_id, account_id, object_key, provider_upload_id,
                storage_class, http_metadata, custom_metadata, ssec_key_md5, ssec_envelope,
                object_version, completion_manifest, completed_metadata, state
         FROM r2_multipart_uploads
         WHERE upload_id = ?1 AND account_id = ?2 AND resource_id = ?3",
            params![upload_id, account_id.to_string(), resource_id.to_string()],
            map_upload,
        )
        .optional()
    {
        Ok(record) => Ok(record),
        Err(rusqlite::Error::InvalidQuery) => Err(invariant()),
        Err(_) => Err(db_error()),
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 multipart authority invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "R2 multipart catalog is unavailable")
}

fn multipart_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2MultipartInvalid,
        "R2 multipart upload is invalid",
    )
}
