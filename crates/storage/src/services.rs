//! Immutable cross-Worker Service declarations and dynamic target authority.

use crate::{ControlDb, DeploymentContentKind, DeploymentState};
use open_compute_core::{AccountId, DeploymentId, ErrorCode, PlatformError, WorkerId};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;

/// One immutable Service declaration frozen into a caller deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentServiceRecord {
    /// Owning caller deployment.
    pub deployment_id: DeploymentId,
    /// Tenant environment binding name.
    pub binding_name: String,
    /// Frozen logical target Worker identity.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    pub entrypoint: Option<String>,
    /// Digest of the canonical Service descriptor.
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Service declaration inserted atomically with a staging deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDeploymentService {
    /// Tenant environment binding name.
    pub binding_name: String,
    /// Existing same-account target Worker identity.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    pub entrypoint: Option<String>,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Dynamically resolved active target identity for one admitted invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedServiceTarget {
    /// Verified immutable declaration.
    pub service: DeploymentServiceRecord,
    /// Same account shared by caller and target.
    pub account_id: AccountId,
    /// Caller Worker owning the declaration.
    pub caller_worker_id: WorkerId,
    /// Active target deployment selected for this call.
    pub target_deployment_id: DeploymentId,
    /// Target descriptor digest required by `RuntimeSource`.
    pub target_worker_code_sha256: [u8; 32],
    /// Current target route generation.
    pub target_route_generation: u64,
    /// Target content discriminator used for fast unsupported-path rejection.
    pub target_content_kind: DeploymentContentKind,
}

/// Bounded inbound declaration shown when target Worker deletion is denied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceReferrer {
    /// Caller Worker.
    pub caller_worker_id: WorkerId,
    /// Retained caller deployment.
    pub caller_deployment_id: DeploymentId,
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
    pub fn deployment_services(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Vec<DeploymentServiceRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_deployment_services_conn(conn, deployment_id))
    }

    /// Re-authorize a declaration and select the target's current active deployment.
    pub fn resolve(
        &self,
        caller_deployment_id: DeploymentId,
        binding_name: &str,
        descriptor_sha256: &[u8; 32],
    ) -> Result<ResolvedServiceTarget, PlatformError> {
        self.db.with_read(|conn| {
            let row = conn
                .query_row(
                    "SELECT s.deployment_id, s.binding_name, s.target_worker_id, s.entrypoint,
                            s.descriptor_sha256, s.created_at_ms,
                            caller.account_id, caller.id, caller.deleted_at_ms, cd.state,
                            target.deleted_at_ms, target.route_generation,
                            td.id, td.content_kind, td.state, td.worker_code_sha256
                     FROM deployment_services s
                     JOIN worker_deployments cd ON cd.id = s.deployment_id
                     JOIN workers caller ON caller.id = cd.worker_id
                     JOIN workers target ON target.id = s.target_worker_id
                     LEFT JOIN worker_deployments td ON td.id = target.active_deployment_id
                     WHERE s.deployment_id = ?1 AND s.binding_name = ?2",
                    params![caller_deployment_id.to_string(), binding_name],
                    |row| {
                        let service = map_service(row)?;
                        let account: String = row.get(6)?;
                        let caller_worker: String = row.get(7)?;
                        let caller_deleted: Option<i64> = row.get(8)?;
                        let caller_state: String = row.get(9)?;
                        let target_deleted: Option<i64> = row.get(10)?;
                        let route_generation: i64 = row.get(11)?;
                        let target_deployment: Option<String> = row.get(12)?;
                        let content_kind: Option<String> = row.get(13)?;
                        let target_state: Option<String> = row.get(14)?;
                        let target_digest: Option<Vec<u8>> = row.get(15)?;
                        Ok((
                            service,
                            account,
                            caller_worker,
                            caller_deleted,
                            caller_state,
                            target_deleted,
                            route_generation,
                            target_deployment,
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
                target_deployment,
                content_kind,
                target_state,
                target_digest,
            )) = row
            else {
                return Err(denied());
            };
            if service.descriptor_sha256 != *descriptor_sha256
                || caller_deleted.is_some()
                || caller_state != DeploymentState::Ready.as_str()
            {
                return Err(denied());
            }
            if target_deleted.is_some()
                || target_state.as_deref() != Some(DeploymentState::Ready.as_str())
            {
                return Err(target_not_ready());
            }
            let target_deployment = target_deployment.ok_or_else(target_not_ready)?;
            let content_kind = content_kind.ok_or_else(target_not_ready)?;
            let target_digest = target_digest.ok_or_else(target_not_ready)?;
            Ok(ResolvedServiceTarget {
                service,
                account_id: AccountId::from_str(&account).map_err(|_| invariant())?,
                caller_worker_id: WorkerId::from_str(&caller_worker).map_err(|_| invariant())?,
                target_deployment_id: DeploymentId::from_str(&target_deployment)
                    .map_err(|_| invariant())?,
                target_worker_code_sha256: target_digest
                    .as_slice()
                    .try_into()
                    .map_err(|_| invariant())?,
                target_route_generation: u64::try_from(route_generation)
                    .map_err(|_| invariant())?,
                target_content_kind: DeploymentContentKind::parse(&content_kind)
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
                     FROM deployment_services s
                     JOIN worker_deployments d ON d.id = s.deployment_id
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
                        let deployment: String = row.get(1)?;
                        Ok(ServiceReferrer {
                            caller_worker_id: WorkerId::from_str(&caller)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            caller_deployment_id: DeploymentId::from_str(&deployment)
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
    deployment_id: DeploymentId,
    services: &[NewDeploymentService],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for service in services {
        tx.execute(
            "INSERT INTO deployment_services
             (deployment_id, binding_name, target_worker_id, entrypoint,
              descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                deployment_id.to_string(),
                service.binding_name,
                service.target_worker_id.to_string(),
                service.entrypoint,
                service.descriptor_sha256.as_slice(),
                now_ms,
            ],
        )
        .map_err(|_| denied())?;
    }
    Ok(())
}

pub(crate) fn read_deployment_services_conn(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<Vec<DeploymentServiceRecord>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT deployment_id, binding_name, target_worker_id, entrypoint,
                    descriptor_sha256, created_at_ms
             FROM deployment_services WHERE deployment_id = ?1 ORDER BY binding_name",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([deployment_id.to_string()], map_service)
        .map_err(|_| db_error())?;
    collect(rows)
}

fn map_service(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentServiceRecord> {
    let deployment: String = row.get(0)?;
    let target: String = row.get(2)?;
    let digest: Vec<u8> = row.get(4)?;
    Ok(DeploymentServiceRecord {
        deployment_id: DeploymentId::from_str(&deployment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        binding_name: row.get(1)?,
        target_worker_id: WorkerId::from_str(&target).map_err(|_| rusqlite::Error::InvalidQuery)?,
        entrypoint: row.get(3)?,
        descriptor_sha256: digest
            .as_slice()
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(5)?,
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
        "Service target has no callable active deployment",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "persisted Service binding invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}
