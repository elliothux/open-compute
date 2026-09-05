//! Durable R2 object metadata authority and external-mutation intents.

use crate::{ControlDb, SecretEnvelope, r2::valid_ssec_key_md5};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr as _;

/// One committed object identity and optional sealed SSE-C key.
#[derive(Clone, Eq, PartialEq)]
pub struct R2ObjectRecord {
    /// Owning logical bucket.
    pub resource_id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Exact tenant object key.
    pub object_key: String,
    /// Platform object version also stored on the provider object.
    pub object_version: String,
    /// Public SSE-C key MD5, when encrypted with SSE-C.
    pub ssec_key_md5: Option<String>,
    /// AEAD envelope for the SSE-C key. Plaintext is never persisted.
    pub ssec_envelope: Option<String>,
}

impl std::fmt::Debug for R2ObjectRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2ObjectRecord")
            .field("resource_id", &self.resource_id)
            .field("account_id", &self.account_id)
            .field("object_key", &self.object_key)
            .field("object_version", &self.object_version)
            .field("ssec_key_md5", &self.ssec_key_md5)
            .field(
                "ssec_envelope",
                &self.ssec_envelope.as_ref().map(|_| "present"),
            )
            .finish()
    }
}

/// Kind of one persisted provider mutation intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum R2ObjectMutationKind {
    /// A provider PUT or multipart completion may be in flight.
    Put,
    /// A provider delete may be in flight.
    Delete,
}

impl R2ObjectMutationKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "put" => Ok(Self::Put),
            "delete" => Ok(Self::Delete),
            _ => Err(invariant()),
        }
    }
}

/// One durable external object-store mutation intent.
#[derive(Clone, Eq, PartialEq)]
pub struct R2ObjectMutationRecord {
    /// Owning logical bucket.
    pub resource_id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Exact tenant object key.
    pub object_key: String,
    /// Mutation kind.
    pub kind: R2ObjectMutationKind,
    /// New object version for a PUT intent.
    pub pending_version: Option<String>,
    /// New public SSE-C key MD5 for a PUT intent.
    pub pending_ssec_key_md5: Option<String>,
    /// New sealed SSE-C key for a PUT intent.
    pub pending_ssec_envelope: Option<String>,
}

/// One logical object or collapsed delimiter prefix in list order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum R2ObjectListEntry {
    /// A committed logical object.
    Object(R2ObjectRecord),
    /// A common prefix ending in the requested delimiter.
    DelimitedPrefix(String),
}

/// One SQLite-authoritative logical R2 list page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2ObjectListPage {
    /// Ordered objects and delimiter prefixes selected for this page.
    pub entries: Vec<R2ObjectListEntry>,
    /// Last raw logical key included by this page when more entries remain.
    pub next_after: Option<String>,
}

impl std::fmt::Debug for R2ObjectMutationRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2ObjectMutationRecord")
            .field("resource_id", &self.resource_id)
            .field("account_id", &self.account_id)
            .field("object_key", &self.object_key)
            .field("kind", &self.kind)
            .field("pending_version", &self.pending_version)
            .field("pending_ssec_key_md5", &self.pending_ssec_key_md5)
            .field(
                "pending_ssec_envelope",
                &self.pending_ssec_envelope.as_ref().map(|_| "present"),
            )
            .finish()
    }
}

/// Typed repository for committed R2 object identities and mutation intents.
#[derive(Clone, Copy, Debug)]
pub struct R2ObjectRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> R2ObjectRepository<'a> {
    /// Bind the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Load one committed object identity.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_key: &str,
    ) -> Result<Option<R2ObjectRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_object(conn, account_id, resource_id, object_key))
    }

    /// Load one pending mutation for an exact object.
    pub fn get_mutation(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_key: &str,
    ) -> Result<Option<R2ObjectMutationRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_mutation(conn, account_id, resource_id, object_key))
    }

    /// List every pending mutation for one logical bucket.
    pub fn list_mutations(
        &self,
        resource_id: ResourceId,
    ) -> Result<Vec<R2ObjectMutationRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT resource_id, account_id, object_key, kind, pending_version,
                            pending_ssec_key_md5, pending_ssec_envelope
                     FROM r2_object_mutations WHERE resource_id = ?1 ORDER BY object_key",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([resource_id.to_string()], map_mutation)
                .map_err(|_| db_error())?;
            rows.map(|row| row.map_err(|_| invariant())).collect()
        })
    }

    /// List committed logical keys in Cloudflare lexicographic order.
    pub fn list(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        prefix: &str,
        delimiter: Option<&str>,
        after: Option<&str>,
        limit: u16,
    ) -> Result<R2ObjectListPage, PlatformError> {
        if limit == 0 || delimiter.is_some_and(str::is_empty) {
            return Err(invariant());
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT resource_id, account_id, object_key, object_version,
                            ssec_key_md5, ssec_envelope
                     FROM r2_objects
                     WHERE resource_id = ?1
                       AND account_id = ?2
                       AND object_key >= ?3
                       AND (?4 IS NULL OR object_key > ?4)
                     ORDER BY object_key",
                )
                .map_err(|_| db_error())?;
            let mut rows = statement
                .query_map(
                    params![
                        resource_id.to_string(),
                        account_id.to_string(),
                        prefix,
                        after,
                    ],
                    map_object,
                )
                .map_err(|_| db_error())?;
            let mut selected: Vec<(R2ObjectListEntry, String)> = Vec::with_capacity(limit.into());
            let mut truncated = false;
            for row in &mut rows {
                let record = row.map_err(|_| invariant())?;
                validate_record(&record)?;
                if !record.object_key.starts_with(prefix) {
                    break;
                }
                let entry = delimiter
                    .and_then(|separator| {
                        record.object_key[prefix.len()..]
                            .find(separator)
                            .map(|position| {
                                R2ObjectListEntry::DelimitedPrefix(
                                    record.object_key[..prefix.len() + position + separator.len()]
                                        .to_owned(),
                                )
                            })
                    })
                    .unwrap_or_else(|| R2ObjectListEntry::Object(record.clone()));
                if let Some((last_entry, last_key)) = selected.last_mut()
                    && list_entry_key(last_entry) == list_entry_key(&entry)
                {
                    *last_key = record.object_key;
                    continue;
                }
                if selected.len() == usize::from(limit) {
                    truncated = true;
                    break;
                }
                selected.push((entry, record.object_key));
            }
            let next_after = truncated
                .then(|| selected.last().map(|(_, last_key)| last_key.clone()))
                .flatten();
            Ok(R2ObjectListPage {
                entries: selected.into_iter().map(|(entry, _)| entry).collect(),
                next_after,
            })
        })
    }

    /// Persist a PUT intent before the external provider mutation begins.
    pub fn begin_put(&self, record: &R2ObjectRecord, now_ms: i64) -> Result<(), PlatformError> {
        validate_record(record)?;
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO r2_object_mutations
                 (resource_id, object_key, account_id, kind, pending_version,
                  pending_ssec_key_md5, pending_ssec_envelope, started_at_ms)
                 VALUES (?1, ?2, ?3, 'put', ?4, ?5, ?6, ?7)",
                params![
                    record.resource_id.to_string(),
                    record.object_key,
                    record.account_id.to_string(),
                    record.object_version,
                    record.ssec_key_md5,
                    record.ssec_envelope,
                    now_ms,
                ],
            )
            .map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Atomically publish a verified PUT and remove its exact intent.
    pub fn finish_put(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_key: &str,
        object_version: &str,
        now_ms: i64,
    ) -> Result<R2ObjectRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let mutation =
                read_mutation(tx, account_id, resource_id, object_key)?.ok_or_else(invariant)?;
            if mutation.kind != R2ObjectMutationKind::Put
                || mutation.pending_version.as_deref() != Some(object_version)
            {
                return Err(invariant());
            }
            tx.execute(
                "INSERT INTO r2_objects
                 (resource_id, object_key, account_id, object_version, ssec_key_md5,
                  ssec_envelope, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(resource_id, object_key) DO UPDATE SET
                   account_id = excluded.account_id,
                   object_version = excluded.object_version,
                   ssec_key_md5 = excluded.ssec_key_md5,
                   ssec_envelope = excluded.ssec_envelope,
                   updated_at_ms = excluded.updated_at_ms",
                params![
                    resource_id.to_string(),
                    object_key,
                    account_id.to_string(),
                    object_version,
                    mutation.pending_ssec_key_md5,
                    mutation.pending_ssec_envelope,
                    now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            delete_mutation(
                tx,
                account_id,
                resource_id,
                object_key,
                R2ObjectMutationKind::Put,
            )?;
            read_object(tx, account_id, resource_id, object_key)?.ok_or_else(invariant)
        })
    }

    /// Remove a proven-not-applied PUT intent without changing committed authority.
    pub fn cancel_put(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_key: &str,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            delete_mutation(
                tx,
                account_id,
                resource_id,
                object_key,
                R2ObjectMutationKind::Put,
            )
        })
    }

    /// Persist delete intents for committed keys before the provider mutation begins.
    pub fn begin_delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_keys: &[String],
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            for object_key in object_keys {
                if read_object(tx, account_id, resource_id, object_key)?.is_none() {
                    return Err(invariant());
                }
                tx.execute(
                    "INSERT INTO r2_object_mutations
                     (resource_id, object_key, account_id, kind, pending_version,
                      pending_ssec_key_md5, pending_ssec_envelope, started_at_ms)
                     VALUES (?1, ?2, ?3, 'delete', NULL, NULL, NULL, ?4)",
                    params![
                        resource_id.to_string(),
                        object_key,
                        account_id.to_string(),
                        now_ms
                    ],
                )
                .map_err(|_| invariant())?;
            }
            Ok(())
        })
    }

    /// Atomically remove committed authority for provider-confirmed deletes and their intents.
    pub fn finish_delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_keys: &[String],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            for object_key in object_keys {
                delete_mutation(
                    tx,
                    account_id,
                    resource_id,
                    object_key,
                    R2ObjectMutationKind::Delete,
                )?;
                if tx
                    .execute(
                        "DELETE FROM r2_objects
                         WHERE resource_id = ?1 AND object_key = ?2 AND account_id = ?3",
                        params![resource_id.to_string(), object_key, account_id.to_string()],
                    )
                    .map_err(|_| db_error())?
                    != 1
                {
                    return Err(invariant());
                }
            }
            Ok(())
        })
    }

    /// Cancel a proven-not-applied delete intent.
    pub fn cancel_delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        object_key: &str,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            delete_mutation(
                tx,
                account_id,
                resource_id,
                object_key,
                R2ObjectMutationKind::Delete,
            )
        })
    }
}

fn list_entry_key(entry: &R2ObjectListEntry) -> &str {
    match entry {
        R2ObjectListEntry::Object(object) => &object.object_key,
        R2ObjectListEntry::DelimitedPrefix(prefix) => prefix,
    }
}

fn validate_record(record: &R2ObjectRecord) -> Result<(), PlatformError> {
    if record.object_version.is_empty()
        || record.ssec_key_md5.is_some() != record.ssec_envelope.is_some()
        || !valid_ssec_key_md5(record.ssec_key_md5.as_deref())
        || record
            .ssec_envelope
            .as_deref()
            .is_some_and(|raw| serde_json::from_str::<SecretEnvelope>(raw).is_err())
    {
        return Err(invariant());
    }
    Ok(())
}

fn read_object(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
    object_key: &str,
) -> Result<Option<R2ObjectRecord>, PlatformError> {
    conn.query_row(
        "SELECT resource_id, account_id, object_key, object_version, ssec_key_md5, ssec_envelope
         FROM r2_objects WHERE resource_id = ?1 AND object_key = ?2 AND account_id = ?3",
        params![resource_id.to_string(), object_key, account_id.to_string()],
        map_object,
    )
    .optional()
    .map_err(|_| db_error())?
    .map_or(Ok(None), |record| {
        validate_record(&record)?;
        Ok(Some(record))
    })
}

fn read_mutation(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
    object_key: &str,
) -> Result<Option<R2ObjectMutationRecord>, PlatformError> {
    conn.query_row(
        "SELECT resource_id, account_id, object_key, kind, pending_version,
                pending_ssec_key_md5, pending_ssec_envelope
         FROM r2_object_mutations
         WHERE resource_id = ?1 AND object_key = ?2 AND account_id = ?3",
        params![resource_id.to_string(), object_key, account_id.to_string()],
        map_mutation,
    )
    .optional()
    .map_err(|_| db_error())?
    .map_or(Ok(None), |record| {
        validate_mutation(&record)?;
        Ok(Some(record))
    })
}

fn map_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<R2ObjectRecord> {
    Ok(R2ObjectRecord {
        resource_id: ResourceId::from_str(&row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(2)?,
        object_version: row.get(3)?,
        ssec_key_md5: row.get(4)?,
        ssec_envelope: row.get(5)?,
    })
}

fn map_mutation(row: &rusqlite::Row<'_>) -> rusqlite::Result<R2ObjectMutationRecord> {
    Ok(R2ObjectMutationRecord {
        resource_id: ResourceId::from_str(&row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(2)?,
        kind: R2ObjectMutationKind::parse(&row.get::<_, String>(3)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        pending_version: row.get(4)?,
        pending_ssec_key_md5: row.get(5)?,
        pending_ssec_envelope: row.get(6)?,
    })
}

fn validate_mutation(record: &R2ObjectMutationRecord) -> Result<(), PlatformError> {
    let put = record.kind == R2ObjectMutationKind::Put;
    if put != record.pending_version.is_some()
        || record.pending_ssec_key_md5.is_some() != record.pending_ssec_envelope.is_some()
        || !valid_ssec_key_md5(record.pending_ssec_key_md5.as_deref())
        || (!put
            && (record.pending_ssec_key_md5.is_some() || record.pending_ssec_envelope.is_some()))
        || record
            .pending_ssec_envelope
            .as_deref()
            .is_some_and(|raw| serde_json::from_str::<SecretEnvelope>(raw).is_err())
    {
        return Err(invariant());
    }
    Ok(())
}

fn delete_mutation(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
    object_key: &str,
    kind: R2ObjectMutationKind,
) -> Result<(), PlatformError> {
    if conn
        .execute(
            "DELETE FROM r2_object_mutations
             WHERE resource_id = ?1 AND object_key = ?2 AND account_id = ?3 AND kind = ?4",
            params![
                resource_id.to_string(),
                object_key,
                account_id.to_string(),
                kind.as_str(),
            ],
        )
        .map_err(|_| db_error())?
        != 1
    {
        return Err(invariant());
    }
    Ok(())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 object metadata authority invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "R2 object metadata authority is unavailable",
    )
}
