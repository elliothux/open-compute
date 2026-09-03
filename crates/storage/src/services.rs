//! Immutable cross-Worker Service declarations and dynamic target authority.

use crate::{ControlDb, VersionContentKind, VersionState};
use open_compute_core::{AccountId, ErrorCode, PlatformError, VersionId, WorkerId};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;

/// One immutable Service declaration frozen into a caller version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionServiceRecord {
    /// Owning caller version.
    pub version_id: VersionId,
    /// Tenant environment binding name.
    pub binding_name: String,
    /// Frozen logical target Worker identity.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    pub entrypoint: Option<String>,
    /// Canonical optional JSON object delivered to the target as `ctx.props`.
    pub props_json: Option<Vec<u8>>,
    /// Digest of the canonical Service descriptor.
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Service declaration inserted atomically with a staging version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewVersionService {
    /// Tenant environment binding name.
    pub binding_name: String,
    /// Existing same-account target Worker identity.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    pub entrypoint: Option<String>,
    /// Canonical optional JSON object delivered to the target as `ctx.props`.
    pub props_json: Option<Vec<u8>>,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Dynamically resolved active target identity for one admitted invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedServiceTarget {
    /// Verified immutable declaration.
    pub service: VersionServiceRecord,
    /// Same account shared by caller and target.
    pub account_id: AccountId,
    /// Caller Worker owning the declaration.
    pub caller_worker_id: WorkerId,
    /// Active target version selected for this call.
    pub target_version_id: VersionId,
    /// Target descriptor digest required by `RuntimeSource`.
    pub target_worker_code_sha256: [u8; 32],
    /// Current target route generation.
    pub target_route_generation: u64,
    /// Target content discriminator used for fast unsupported-path rejection.
    pub target_content_kind: VersionContentKind,
}

/// Bounded inbound declaration shown when target Worker deletion is denied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceReferrer {
    /// Caller Worker.
    pub caller_worker_id: WorkerId,
    /// Retained caller version.
    pub caller_version_id: VersionId,
    /// Caller environment name.
    pub binding_name: String,
}

/// Repository for persisted Service declarations.
#[derive(Clone, Copy, Debug)]
pub struct ServiceRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> ServiceRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read declarations in canonical binding-name order.
    pub fn version_services(
        &self,
        version_id: VersionId,
    ) -> Result<Vec<VersionServiceRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_version_services_conn(conn, version_id))
    }

    /// Re-authorize a declaration and select the target's current active version.
    pub fn resolve(
        &self,
        caller_version_id: VersionId,
        binding_name: &str,
        descriptor_sha256: &[u8; 32],
    ) -> Result<ResolvedServiceTarget, PlatformError> {
        self.db.with_read(|conn| {
            let row = conn
                .query_row(
                    "SELECT s.version_id, s.binding_name, s.target_worker_id, s.entrypoint,
                            s.props_json, s.descriptor_sha256, s.created_at_ms,
                            caller.account_id, caller.id, caller.deleted_at_ms, cd.state,
                            target.deleted_at_ms, target.route_generation,
                            td.id, td.content_kind, td.state, td.worker_code_sha256
                     FROM version_services s
                     JOIN worker_versions cd ON cd.id = s.version_id
                     JOIN workers caller ON caller.id = cd.worker_id
                     JOIN workers target ON target.id = s.target_worker_id
                     LEFT JOIN worker_deployments active ON active.id = target.active_deployment_id
                     LEFT JOIN worker_versions td ON td.id = active.version_id
                     WHERE s.version_id = ?1 AND s.binding_name = ?2",
                    params![caller_version_id.to_string(), binding_name],
                    |row| {
                        let service = map_service(row)?;
                        let account: String = row.get(7)?;
                        let caller_worker: String = row.get(8)?;
                        let caller_deleted: Option<i64> = row.get(9)?;
                        let caller_state: String = row.get(10)?;
                        let target_deleted: Option<i64> = row.get(11)?;
                        let route_generation: i64 = row.get(12)?;
                        let target_version: Option<String> = row.get(13)?;
                        let content_kind: Option<String> = row.get(14)?;
                        let target_state: Option<String> = row.get(15)?;
                        let target_digest: Option<Vec<u8>> = row.get(16)?;
                        Ok((
                            service,
                            account,
                            caller_worker,
                            caller_deleted,
                            caller_state,
                            target_deleted,
                            route_generation,
                            target_version,
                            content_kind,
                            target_state,
                            target_digest,
                        ))
                    },
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((
                service,
                account,
                caller_worker,
                caller_deleted,
                caller_state,
                target_deleted,
                route_generation,
                target_version,
                content_kind,
                target_state,
                target_digest,
            )) = row
            else {
                return Err(denied());
            };
            if service.descriptor_sha256 != *descriptor_sha256
                || caller_deleted.is_some()
                || caller_state != VersionState::Ready.as_str()
            {
                return Err(denied());
            }
            if target_deleted.is_some()
                || target_state.as_deref() != Some(VersionState::Ready.as_str())
            {
                return Err(target_not_ready());
            }
            let target_version = target_version.ok_or_else(target_not_ready)?;
            let content_kind = content_kind.ok_or_else(target_not_ready)?;
            let target_digest = target_digest.ok_or_else(target_not_ready)?;
            Ok(ResolvedServiceTarget {
                service,
                account_id: AccountId::from_str(&account).map_err(|_| invariant())?,
                caller_worker_id: WorkerId::from_str(&caller_worker).map_err(|_| invariant())?,
                target_version_id: VersionId::from_str(&target_version).map_err(|_| invariant())?,
                target_worker_code_sha256: target_digest
                    .as_slice()
                    .try_into()
                    .map_err(|_| invariant())?,
                target_route_generation: u64::try_from(route_generation)
                    .map_err(|_| invariant())?,
                target_content_kind: VersionContentKind::parse(&content_kind)
                    .map_err(|_| invariant())?,
            })
        })
    }

    /// Return effective inbound declarations from other Workers.
    pub fn inbound_referrers(
        &self,
        account_id: AccountId,
        target_worker_id: WorkerId,
        limit: u32,
    ) -> Result<Vec<ServiceReferrer>, PlatformError> {
        if limit == 0 || limit > 1_000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Service referrer query limit is invalid",
            ));
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT caller.id, d.id, s.binding_name
                     FROM version_services s
                     JOIN worker_versions d ON d.id = s.version_id
                     JOIN workers caller ON caller.id = d.worker_id
                     JOIN workers target ON target.id = s.target_worker_id
                     WHERE s.target_worker_id = ?1
                       AND target.account_id = ?2
                       AND caller.account_id = target.account_id
                       AND caller.id != target.id
                       AND caller.deleted_at_ms IS NULL
                       AND d.state IN ('staging', 'validating', 'ready')
                     ORDER BY caller.id, d.id, s.binding_name LIMIT ?3",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map(
                    params![
                        target_worker_id.to_string(),
                        account_id.to_string(),
                        i64::from(limit)
                    ],
                    |row| {
                        let caller: String = row.get(0)?;
                        let version: String = row.get(1)?;
                        Ok(ServiceReferrer {
                            caller_worker_id: WorkerId::from_str(&caller)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            caller_version_id: VersionId::from_str(&version)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            binding_name: row.get(2)?,
                        })
                    },
                )
                .map_err(|_| db_error())?;
            collect(rows)
        })
    }
}

pub(crate) fn insert_staging_services(
    tx: &Transaction<'_>,
    version_id: VersionId,
    services: &[NewVersionService],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for service in services {
        tx.execute(
            "INSERT INTO version_services
             (version_id, binding_name, target_worker_id, entrypoint, props_json,
              descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                version_id.to_string(),
                service.binding_name,
                service.target_worker_id.to_string(),
                service.entrypoint,
                service.props_json,
                service.descriptor_sha256.as_slice(),
                now_ms,
            ],
        )
        .map_err(|_| denied())?;
    }
    Ok(())
}

pub(crate) fn read_version_services_conn(
    conn: &rusqlite::Connection,
    version_id: VersionId,
) -> Result<Vec<VersionServiceRecord>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT version_id, binding_name, target_worker_id, entrypoint, props_json,
                    descriptor_sha256, created_at_ms
             FROM version_services WHERE version_id = ?1 ORDER BY binding_name",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([version_id.to_string()], map_service)
        .map_err(|_| db_error())?;
    collect(rows)
}

fn map_service(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionServiceRecord> {
    let version: String = row.get(0)?;
    let target: String = row.get(2)?;
    let digest: Vec<u8> = row.get(5)?;
    Ok(VersionServiceRecord {
        version_id: VersionId::from_str(&version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        binding_name: row.get(1)?,
        target_worker_id: WorkerId::from_str(&target).map_err(|_| rusqlite::Error::InvalidQuery)?,
        entrypoint: row.get(3)?,
        props_json: row.get(4)?,
        descriptor_sha256: digest
            .as_slice()
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(6)?,
    })
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut values = Vec::new();
    for row in rows {
        values.push(row.map_err(|_| invariant())?);
    }
    Ok(values)
}

fn denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceBindingDenied,
        "Service binding authority denied the invocation",
    )
}

fn target_not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceTargetNotReady,
        "Service target has no callable active version",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted Service binding invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}
