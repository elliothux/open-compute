//! Typed P0.2 control-plane repository.

use crate::{ControlDb, SecretEnvelope};
use open_compute_core::{AccountId, DeploymentId, ErrorCode, PlatformError, RequestId, WorkerId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::str::FromStr;
use uuid::Uuid;

mod deployment_create;
pub use deployment_create::NewDeploymentProducts;

/// Current immutable loader descriptor schema.
pub const LOADER_SCHEMA_VERSION: i64 = 1;

/// Persisted Worker lifecycle row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    /// Opaque Worker identity.
    pub id: WorkerId,
    /// Owning account.
    pub account_id: AccountId,
    /// Lowercase display slug.
    pub name: String,
    /// Current active immutable deployment.
    pub active_deployment_id: Option<DeploymentId>,
    /// Stable future Durable Object storage identity.
    pub do_storage_id: String,
    /// Route/promotion generation.
    pub route_generation: u64,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last mutation timestamp.
    pub updated_at_ms: i64,
    /// Tombstone timestamp.
    pub deleted_at_ms: Option<i64>,
}

/// Deployment lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    /// Metadata and env are being inserted.
    Staging,
    /// Real workerd validation is in progress.
    Validating,
    /// Immutable deployment may be dispatched or promoted.
    Ready,
    /// Validation deterministically failed.
    Rejected,
    /// New pins are fenced while references drain.
    Deleting,
    /// Metadata is no longer dispatchable.
    Tombstoned,
}

/// Executable or static-only content carried by an immutable deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentContentKind {
    /// Tenant Worker code, with optional static assets.
    Worker,
    /// Static assets without a fabricated tenant Worker.
    AssetsOnly,
}

impl DeploymentContentKind {
    /// Stable current-schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::AssetsOnly => "assets_only",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "worker" => Ok(Self::Worker),
            "assets_only" => Ok(Self::AssetsOnly),
            _ => Err(invariant()),
        }
    }
}

impl DeploymentState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Validating => "validating",
            Self::Ready => "ready",
            Self::Rejected => "rejected",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "staging" => Ok(Self::Staging),
            "validating" => Ok(Self::Validating),
            "ready" => Ok(Self::Ready),
            "rejected" => Ok(Self::Rejected),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Persisted immutable deployment metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentRecord {
    /// Deployment identity.
    pub id: DeploymentId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Monotonic Worker-local version.
    pub version_number: u64,
    /// Deployment content union discriminator.
    pub content_kind: DeploymentContentKind,
    /// Lifecycle state.
    pub state: DeploymentState,
    /// Canonical bundle digest.
    pub artifact_sha256: Option<[u8; 32]>,
    /// Canonical bundle size.
    pub artifact_size: Option<u64>,
    /// Artifact framing schema.
    pub artifact_schema_version: Option<u32>,
    /// Main ES module.
    pub main_module: Option<String>,
    /// Tenant compatibility date.
    pub compatibility_date: String,
    /// Canonically sorted compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// Immutable limits document.
    pub limits: serde_json::Value,
    /// Hash of every runtime-effective input.
    pub worker_code_sha256: [u8; 32],
    /// Loader contract schema.
    pub loader_schema_version: u32,
    /// Creation time.
    pub created_at_ms: i64,
    /// Ready time.
    pub ready_at_ms: Option<i64>,
    /// Rejection time.
    pub rejected_at_ms: Option<i64>,
    /// Stable rejection code.
    pub rejection_code: Option<String>,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

/// Secret ciphertext stored for one immutable deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredDeploymentSecret {
    /// Environment name.
    pub name: String,
    /// Immutable random revision.
    pub revision_id: String,
    /// AEAD envelope.
    pub envelope: SecretEnvelope,
}

/// Consistent immutable source snapshot used by `RuntimeSource`.
#[derive(Clone, Debug, PartialEq)]
pub struct DeploymentSnapshot {
    /// Account identity.
    pub account_id: AccountId,
    /// Worker row.
    pub worker: WorkerRecord,
    /// Deployment row.
    pub deployment: DeploymentRecord,
    /// Static-asset authority when the deployment declares assets.
    pub assets: Option<crate::DeploymentAssetsRecord>,
    /// Canonical JSON vars keyed by env name.
    pub vars: BTreeMap<String, Vec<u8>>,
    /// Encrypted secrets keyed by env name.
    pub secrets: BTreeMap<String, StoredDeploymentSecret>,
    /// Immutable typed resource bindings ordered by env name.
    pub bindings: Vec<crate::DeploymentBindingRecord>,
    /// Immutable Queue producer bindings ordered by env name.
    pub queue_bindings: Vec<crate::QueueProducerBindingRecord>,
    /// Immutable Workflow caller bindings ordered by env name.
    pub workflow_bindings: Vec<crate::WorkflowBindingRecord>,
}

/// Route kind supported by P0.2.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    /// Platform-owned account/worker path.
    PlatformPath,
    /// Exact canonical hostname plus path prefix.
    ExactHost,
}

impl RouteKind {
    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "platform_path" => Ok(Self::PlatformPath),
            "exact_host" => Ok(Self::ExactHost),
            _ => Err(invariant()),
        }
    }
}

/// Active route metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRecord {
    /// Opaque route identity.
    pub id: String,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Route kind.
    pub kind: RouteKind,
    /// Canonical exact hostname.
    pub hostname_ascii: Option<String>,
    /// Canonical path prefix.
    pub path_prefix: String,
    /// Optional named entrypoint.
    pub entrypoint: Option<String>,
    /// Route generation at creation/update.
    pub generation: u64,
}

/// Frozen route and active deployment identity for one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteSnapshot {
    /// Matched route.
    pub route: RouteRecord,
    /// Matched Worker.
    pub worker: WorkerRecord,
    /// Active ready deployment.
    pub deployment: DeploymentRecord,
    /// Static-asset authority frozen with the same active deployment.
    pub assets: Option<crate::DeploymentAssetsRecord>,
}

/// Registered reason a deployment must remain reachable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentReferrer {
    /// Immutable deployment identity.
    pub deployment_id: DeploymentId,
    /// Owning subsystem token such as `control_idempotency`.
    pub kind: String,
    /// Stable subsystem-local reference identity.
    pub ref_id: String,
    /// Registration timestamp.
    pub created_at_ms: i64,
}

/// One non-active, unreferenced deployment eligible for automatic retention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionCandidate {
    /// Account boundary.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Candidate deployment.
    pub deployment_id: DeploymentId,
}

/// Input for an immutable staging deployment transaction.
#[derive(Clone, Debug)]
pub struct NewDeployment {
    /// Platform-generated identity.
    pub id: DeploymentId,
    /// Owning account.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Deployment content union discriminator.
    pub content_kind: DeploymentContentKind,
    /// Artifact digest.
    pub artifact_sha256: Option<[u8; 32]>,
    /// Artifact size.
    pub artifact_size: Option<u64>,
    /// Artifact schema.
    pub artifact_schema_version: Option<u32>,
    /// Main module.
    pub main_module: Option<String>,
    /// Tenant compatibility date.
    pub compatibility_date: String,
    /// Sorted flags.
    pub compatibility_flags: Vec<String>,
    /// Immutable limits.
    pub limits: serde_json::Value,
    /// Descriptor digest.
    pub worker_code_sha256: [u8; 32],
    /// Canonical JSON vars.
    pub vars: BTreeMap<String, Vec<u8>>,
    /// Encrypted secret rows.
    pub secrets: BTreeMap<String, StoredDeploymentSecret>,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Transaction timestamp.
    pub now_ms: i64,
}

/// Idempotency reservation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyReservation {
    /// Caller owns a newly inserted running row.
    Reserved,
    /// Same canonical request has already completed.
    Complete(Vec<u8>),
    /// Same canonical request is already running.
    Running,
    /// Same canonical request previously failed; value is the stable response envelope.
    Failed(Vec<u8>),
}

/// Central typed repository. The raw `SQLite` connection remains private.
#[derive(Clone, Copy, Debug)]
pub struct WorkerRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> WorkerRepository<'a> {
    /// Bind the repository to an already migrated control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Create a Worker while atomically enforcing the account live-Worker limit.
    pub fn create_worker(
        &self,
        account_id: AccountId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
        max_live: u32,
    ) -> Result<(WorkerRecord, RouteRecord), PlatformError> {
        validate_worker_name(name)?;
        if max_live == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Worker count limit must be greater than zero",
            ));
        }
        let worker_id = WorkerId::generate();
        let do_storage_id = Uuid::now_v7().to_string();
        let route_id = Uuid::now_v7().to_string();
        let prefix = format!("/__workers/{account_id}/{name}/");
        self.db.with_immediate(|tx| {
            require_account(tx, account_id)?;
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM workers
                     WHERE account_id = ?1 AND deleted_at_ms IS NULL",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if live_count >= i64::from(max_live) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "account Worker count quota was exceeded",
                ));
            }
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO workers
                 (id, account_id, name, active_deployment_id, do_storage_id,
                  route_generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, NULL, ?4, 1, ?5, ?5, NULL)",
                    params![
                        worker_id.to_string(),
                        account_id.to_string(),
                        name,
                        do_storage_id,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
            if inserted != 1 {
                return Err(PlatformError::new(
                    ErrorCode::WorkerNameConflict,
                    "a live Worker already owns this name",
                ));
            }
            tx.execute(
                "INSERT INTO worker_routes
                 (id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                  entrypoint, state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, 'platform_path', NULL, ?4, NULL,
                         'active', 1, ?5, ?5, NULL)",
                params![
                    route_id,
                    account_id.to_string(),
                    worker_id.to_string(),
                    prefix,
                    now_ms
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "worker.create",
                "worker",
                &worker_id.to_string(),
                request_id,
                br#"{"state":"live"}"#,
                now_ms,
            )?;
            let worker = WorkerRecord {
                id: worker_id,
                account_id,
                name: name.to_owned(),
                active_deployment_id: None,
                do_storage_id: do_storage_id.clone(),
                route_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                deleted_at_ms: None,
            };
            let route = RouteRecord {
                id: route_id.clone(),
                account_id,
                worker_id,
                kind: RouteKind::PlatformPath,
                hostname_ascii: None,
                path_prefix: prefix.clone(),
                entrypoint: None,
                generation: 1,
            };
            Ok((worker, route))
        })
    }

    /// List live Workers in deterministic creation order.
    pub fn list_workers(&self, account_id: AccountId) -> Result<Vec<WorkerRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, account_id, name, active_deployment_id, do_storage_id,
                        route_generation, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM workers WHERE account_id = ?1 AND deleted_at_ms IS NULL
                 ORDER BY created_at_ms, id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([account_id.to_string()], map_worker)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Read one Worker and enforce its account boundary.
    pub fn get_worker(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<WorkerRecord, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT id, account_id, name, active_deployment_id, do_storage_id,
                        route_generation, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM workers WHERE id = ?1 AND account_id = ?2",
                params![worker_id.to_string(), account_id.to_string()],
                map_worker,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(worker_not_found)
        })
    }

    /// Transition staging to validating.
    pub fn begin_validation(&self, deployment_id: DeploymentId) -> Result<(), PlatformError> {
        self.transition(
            deployment_id,
            DeploymentState::Staging,
            DeploymentState::Validating,
            0,
            None,
        )
    }

    /// Mark a validating deployment ready.
    pub fn mark_ready(
        &self,
        deployment_id: DeploymentId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.transition(
            deployment_id,
            DeploymentState::Validating,
            DeploymentState::Ready,
            now_ms,
            None,
        )
    }

    /// Reject a staging or validating deployment with a stable safe code.
    pub fn mark_rejected(
        &self,
        deployment_id: DeploymentId,
        expected: DeploymentState,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !matches!(
            expected,
            DeploymentState::Staging | DeploymentState::Validating
        ) {
            return Err(invariant());
        }
        self.transition(
            deployment_id,
            expected,
            DeploymentState::Rejected,
            now_ms,
            Some(code.as_str()),
        )
    }

    fn transition(
        self,
        deployment_id: DeploymentId,
        expected: DeploymentState,
        target: DeploymentState,
        now_ms: i64,
        rejection_code: Option<&str>,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE worker_deployments
                 SET state = ?1,
                     ready_at_ms = CASE WHEN ?1 = 'ready' THEN ?2 ELSE ready_at_ms END,
                     rejected_at_ms = CASE WHEN ?1 = 'rejected' THEN ?2 ELSE rejected_at_ms END,
                     rejection_code = CASE WHEN ?1 = 'rejected' THEN ?3 ELSE rejection_code END
                 WHERE id = ?4 AND state = ?5",
                    params![
                        target.as_str(),
                        now_ms,
                        rejection_code,
                        deployment_id.to_string(),
                        expected.as_str()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentNotReady,
                    "deployment state transition precondition failed",
                ));
            }
            Ok(())
        })
    }

    /// Atomically promote a ready deployment, optionally using compare-and-swap.
    pub fn promote(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: DeploymentId,
        expected_active: Option<DeploymentId>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.promote_checked(
            account_id,
            worker_id,
            target,
            expected_active,
            None,
            request_id,
            now_ms,
        )
    }

    /// Promote only if both the optional active pointer and route generation still match.
    #[allow(clippy::too_many_arguments)]
    pub fn promote_checked(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        target: DeploymentId,
        expected_active: Option<DeploymentId>,
        expected_route_generation: Option<u64>,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<WorkerRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let current = require_live_worker(tx, account_id, worker_id)?;
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_deployments WHERE id = ?1 AND worker_id = ?2",
                    params![target.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some(state) = state else {
                return Err(deployment_not_found());
            };
            if state != "ready" {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentNotReady,
                    "promotion target is not a ready deployment of this Worker",
                ));
            }
            if expected_active
                .is_some_and(|expected| current.active_deployment_id != Some(expected))
                || expected_route_generation
                    .is_some_and(|expected| current.route_generation != expected)
            {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "promotion compare-and-swap precondition failed",
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id = ?1,
                         route_generation = route_generation + 1, updated_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND deleted_at_ms IS NULL
                       AND route_generation = ?5",
                    params![
                        target.to_string(),
                        now_ms,
                        worker_id.to_string(),
                        account_id.to_string(),
                        i64::try_from(current.route_generation).map_err(|_| invariant())?
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "promotion compare-and-swap precondition failed",
                ));
            }
            audit(
                tx,
                account_id,
                "deployment.promote",
                "deployment",
                &target.to_string(),
                request_id,
                br#"{"state":"active"}"#,
                now_ms,
            )?;
            read_worker_tx(tx, account_id, worker_id)
        })
    }

    /// Read an immutable deployment with vars and secret ciphertext in one snapshot.
    pub fn deployment_snapshot(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        allow_validating: bool,
    ) -> Result<DeploymentSnapshot, PlatformError> {
        self.db.with_read(|conn| {
            let worker = conn
                .query_row(
                    "SELECT id, account_id, name, active_deployment_id, do_storage_id,
                        route_generation, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM workers WHERE id = ?1 AND account_id = ?2",
                    params![worker_id.to_string(), account_id.to_string()],
                    map_worker,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(worker_not_found)?;
            if worker.deleted_at_ms.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::WorkerDeleted,
                    "Worker is tombstoned",
                ));
            }
            let deployment = conn
                .query_row(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        compatibility_date, compatibility_flags_json, limits_json,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms
                 FROM worker_deployments WHERE id = ?1 AND worker_id = ?2",
                    params![deployment_id.to_string(), worker_id.to_string()],
                    map_deployment,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(deployment_not_found)?;
            if deployment.state != DeploymentState::Ready
                && !(allow_validating && deployment.state == DeploymentState::Validating)
            {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentNotReady,
                    "deployment is not available to RuntimeSource",
                ));
            }
            let vars = read_vars(conn, deployment_id)?;
            let secrets = read_secrets(conn, deployment_id)?;
            let bindings = crate::bindings::read_deployment_bindings_conn(conn, deployment_id)?;
            let queue_bindings = crate::queues::read_deployment_bindings_conn(conn, deployment_id)?;
            Ok(DeploymentSnapshot {
                account_id,
                worker,
                assets: crate::assets::read_assets_conn(conn, deployment_id)?,
                deployment,
                vars,
                secrets,
                bindings,
                queue_bindings,
                workflow_bindings: crate::workflows::bindings::read_workflow_bindings(
                    conn,
                    deployment_id,
                )?,
            })
        })
    }

    /// List all deployments, newest first.
    pub fn list_deployments(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<DeploymentRecord>, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        compatibility_date, compatibility_flags_json, limits_json,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms
                 FROM worker_deployments WHERE worker_id = ?1
                 ORDER BY version_number DESC",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([worker_id.to_string()], map_deployment)
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Read one deployment while enforcing the account and Worker boundary.
    pub fn get_deployment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
    ) -> Result<DeploymentRecord, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            conn.query_row(
                "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        compatibility_date, compatibility_flags_json, limits_json,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms
                 FROM worker_deployments WHERE id = ?1 AND worker_id = ?2",
                params![deployment_id.to_string(), worker_id.to_string()],
                map_deployment,
            )
            .optional()
            .map_err(|_| db_error())?
            .ok_or_else(deployment_not_found)
        })
    }

    /// Resolve the longest active exact-host or platform path route and freeze active deployment.
    pub fn resolve_route(
        &self,
        hostname_ascii: Option<&str>,
        path: &str,
    ) -> Result<RouteSnapshot, PlatformError> {
        self.db.with_read(|conn| {
            let sql = if hostname_ascii.is_some() {
                "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                        entrypoint, generation
                 FROM worker_routes
                 WHERE kind = 'exact_host' AND hostname_ascii = ?1 AND state = 'active'
                   AND ?2 LIKE path_prefix || '%'
                 ORDER BY length(path_prefix) DESC LIMIT 1"
            } else {
                "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                        entrypoint, generation
                 FROM worker_routes
                 WHERE kind = 'platform_path' AND state = 'active'
                   AND ?2 LIKE path_prefix || '%'
                 ORDER BY length(path_prefix) DESC LIMIT 1"
            };
            let route = conn
                .query_row(sql, params![hostname_ascii.unwrap_or(""), path], map_route)
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            let worker = conn
                .query_row(
                    "SELECT id, account_id, name, active_deployment_id, do_storage_id,
                        route_generation, created_at_ms, updated_at_ms, deleted_at_ms
                 FROM workers WHERE id = ?1 AND account_id = ?2 AND deleted_at_ms IS NULL",
                    params![route.worker_id.to_string(), route.account_id.to_string()],
                    map_worker,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            let active = worker.active_deployment_id.ok_or_else(route_not_found)?;
            let deployment = conn
                .query_row(
                    "SELECT id, worker_id, version_number, content_kind, state, artifact_sha256,
                        artifact_size, artifact_schema_version, main_module,
                        compatibility_date, compatibility_flags_json, limits_json,
                        worker_code_sha256, loader_schema_version, created_at_ms,
                        ready_at_ms, rejected_at_ms, rejection_code, deleted_at_ms
                 FROM worker_deployments WHERE id = ?1 AND worker_id = ?2 AND state = 'ready'",
                    params![active.to_string(), worker.id.to_string()],
                    map_deployment,
                )
                .optional()
                .map_err(|_| db_error())?
                .ok_or_else(route_not_found)?;
            Ok(RouteSnapshot {
                route,
                worker,
                assets: crate::assets::read_assets_conn(conn, active)?,
                deployment,
            })
        })
    }

    /// Add an exact-host route while atomically enforcing the account route limit.
    #[allow(clippy::too_many_arguments)]
    pub fn create_exact_route(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        hostname_ascii: &str,
        path_prefix: &str,
        entrypoint: Option<&str>,
        expected_active: Option<DeploymentId>,
        request_id: RequestId,
        now_ms: i64,
        max_live: u32,
    ) -> Result<RouteRecord, PlatformError> {
        validate_exact_route(hostname_ascii, path_prefix, entrypoint)?;
        if max_live == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "route count limit must be greater than zero",
            ));
        }
        let route_id = Uuid::now_v7().to_string();
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM worker_routes
                     WHERE account_id = ?1 AND state = 'active' AND deleted_at_ms IS NULL",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if live_count >= i64::from(max_live) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "account route count quota was exceeded",
                ));
            }
            if expected_active.is_some_and(|expected| worker.active_deployment_id != Some(expected))
            {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "route entrypoint probe snapshot changed",
                ));
            }
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO worker_routes
                 (id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                  entrypoint, state, generation, created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, 'exact_host', ?4, ?5, ?6,
                         'active', ?7, ?8, ?8, NULL)",
                    params![
                        route_id,
                        account_id.to_string(),
                        worker_id.to_string(),
                        hostname_ascii,
                        path_prefix,
                        entrypoint,
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms
                    ],
                )
                .map_err(|_| db_error())?;
            if inserted != 1 {
                return Err(PlatformError::new(
                    ErrorCode::RouteConflict,
                    "an active route already owns this host and path prefix",
                ));
            }
            tx.execute(
                "UPDATE workers SET route_generation = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "route.create",
                "route",
                &route_id,
                request_id,
                br#"{"state":"active"}"#,
                now_ms,
            )?;
            Ok(RouteRecord {
                id: route_id.clone(),
                account_id,
                worker_id,
                kind: RouteKind::ExactHost,
                hostname_ascii: Some(hostname_ascii.to_owned()),
                path_prefix: path_prefix.to_owned(),
                entrypoint: entrypoint.map(ToOwned::to_owned),
                generation,
            })
        })
    }

    /// List active routes owned by one live Worker.
    pub fn list_routes(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
    ) -> Result<Vec<RouteRecord>, PlatformError> {
        self.get_worker(account_id, worker_id)?;
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, account_id, worker_id, kind, hostname_ascii, path_prefix,
                            entrypoint, generation
                     FROM worker_routes
                     WHERE account_id = ?1 AND worker_id = ?2 AND state = 'active'
                     ORDER BY kind, hostname_ascii, path_prefix, id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map(
                    params![account_id.to_string(), worker_id.to_string()],
                    map_route,
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Tombstone one exact-host route and increment the Worker route generation.
    pub fn delete_route(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        route_id: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            let changed = tx
                .execute(
                    "UPDATE worker_routes
                     SET state = 'tombstoned', generation = ?1, updated_at_ms = ?2,
                         deleted_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND worker_id = ?5
                       AND kind = 'exact_host' AND state = 'active'",
                    params![
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms,
                        route_id,
                        account_id.to_string(),
                        worker_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(route_not_found());
            }
            tx.execute(
                "UPDATE workers SET route_generation = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            audit(
                tx,
                account_id,
                "route.delete",
                "route",
                route_id,
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )
        })
    }

    /// Atomically disable routes, clear active deployment, and tombstone a Worker.
    pub fn delete_worker(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            let generation = worker
                .route_generation
                .checked_add(1)
                .ok_or_else(invariant)?;
            tx.execute(
                "UPDATE worker_routes SET state = 'tombstoned', generation = ?1,
                        updated_at_ms = ?2, deleted_at_ms = ?2
                 WHERE account_id = ?3 AND worker_id = ?4 AND state = 'active'",
                params![
                    i64::try_from(generation).map_err(|_| invariant())?,
                    now_ms,
                    account_id.to_string(),
                    worker_id.to_string()
                ],
            )
            .map_err(|_| db_error())?;
            let changed = tx
                .execute(
                    "UPDATE workers SET active_deployment_id = NULL,
                            route_generation = ?1, updated_at_ms = ?2, deleted_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND deleted_at_ms IS NULL",
                    params![
                        i64::try_from(generation).map_err(|_| invariant())?,
                        now_ms,
                        worker_id.to_string(),
                        account_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(worker_not_found());
            }
            audit(
                tx,
                account_id,
                "worker.delete",
                "worker",
                &worker_id.to_string(),
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )
        })
    }

    /// Reserve or replay a secret-safe control idempotency key.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_idempotency(
        &self,
        account_id: AccountId,
        scope: &str,
        key: &str,
        fingerprint_key_id: &str,
        fingerprint: &[u8; 32],
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<IdempotencyReservation, PlatformError> {
        self.db.with_immediate(|tx| {
            let existing: Option<(String, Vec<u8>, Option<Vec<u8>>)> = tx
                .query_row(
                    "SELECT state, request_fingerprint, response_json
                 FROM control_idempotency
                 WHERE account_id = ?1 AND scope = ?2 AND idempotency_key = ?3",
                    params![account_id.to_string(), scope, key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((state, stored, response)) = existing {
                if stored.as_slice() != fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "idempotency key fingerprint does not match",
                    ));
                }
                return match (state.as_str(), response) {
                    ("complete", Some(body)) => Ok(IdempotencyReservation::Complete(body)),
                    ("running", _) => Ok(IdempotencyReservation::Running),
                    ("failed", Some(body)) => Ok(IdempotencyReservation::Failed(body)),
                    _ => Err(PlatformError::new(
                        ErrorCode::Internal,
                        "idempotency row is in an invalid state",
                    )),
                };
            }
            tx.execute(
                "INSERT INTO control_idempotency
                 (account_id, scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, state, created_at_ms, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'running', ?6, ?7)",
                params![
                    account_id.to_string(),
                    scope,
                    key,
                    fingerprint_key_id,
                    fingerprint.as_slice(),
                    now_ms,
                    expires_at_ms
                ],
            )
            .map_err(|_| db_error())?;
            Ok(IdempotencyReservation::Reserved)
        })
    }

    /// Persist the canonical response for an owned idempotency reservation.
    pub fn complete_idempotency(
        &self,
        account_id: AccountId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1
                 WHERE account_id = ?2 AND scope = ?3 AND idempotency_key = ?4
                   AND state = 'running' AND request_fingerprint = ?5",
                    params![
                        response,
                        account_id.to_string(),
                        scope,
                        key,
                        fingerprint.as_slice()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Complete an idempotent Queue mutation and retain its exact Queue identity.
    pub fn complete_idempotency_with_queue_ref(
        &self,
        account_id: AccountId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
        queue_id: open_compute_core::QueueId,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1,
                            queue_id = ?6
                     WHERE account_id = ?2 AND scope = ?3 AND idempotency_key = ?4
                       AND state = 'running' AND request_fingerprint = ?5
                       AND (queue_id IS NULL OR queue_id = ?6)",
                    params![
                        response,
                        account_id.to_string(),
                        scope,
                        key,
                        fingerprint.as_slice(),
                        queue_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Complete an idempotent response and register its deployment readback ref atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_idempotency_with_deployment_ref(
        &self,
        account_id: AccountId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
        deployment_id: DeploymentId,
        ref_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_referrer("control_idempotency", ref_id)?;
        if ref_id != idempotency_ref_id(account_id, scope, key) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1,
                            deployment_id = ?6
                     WHERE account_id = ?2 AND scope = ?3 AND idempotency_key = ?4
                       AND state = 'running' AND request_fingerprint = ?5",
                    params![
                        response,
                        account_id.to_string(),
                        scope,
                        key,
                        fingerprint.as_slice(),
                        deployment_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO deployment_referrers
                 (deployment_id, kind, ref_id, created_at_ms)
                 VALUES (?1, 'control_idempotency', ?2, ?3)",
                params![deployment_id.to_string(), ref_id, now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Persist a stable secret-safe failure for deterministic replay.
    pub fn fail_idempotency(
        &self,
        account_id: AccountId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'failed', response_json = ?1
                 WHERE account_id = ?2 AND scope = ?3 AND idempotency_key = ?4
                   AND state = 'running' AND request_fingerprint = ?5",
                    params![
                        response,
                        account_id.to_string(),
                        scope,
                        key,
                        fingerprint.as_slice()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Register a typed deployment referrer. Future products must use this table.
    pub fn add_deployment_referrer(
        &self,
        deployment_id: DeploymentId,
        kind: &str,
        ref_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_referrer(kind, ref_id)?;
        self.db.with_immediate(|tx| {
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_deployments WHERE id = ?1",
                    [deployment_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            if !state.is_some_and(|state| matches!(state.as_str(), "ready" | "rejected")) {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentNotReady,
                    "deployment cannot accept a referrer in its current state",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO deployment_referrers
                 (deployment_id, kind, ref_id, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![deployment_id.to_string(), kind, ref_id, now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Remove a typed deployment referrer after its owner no longer needs replay/readback.
    pub fn remove_deployment_referrer(
        &self,
        deployment_id: DeploymentId,
        kind: &str,
        ref_id: &str,
    ) -> Result<(), PlatformError> {
        validate_referrer(kind, ref_id)?;
        self.db.with_immediate(|tx| {
            tx.execute(
                "DELETE FROM deployment_referrers
                 WHERE deployment_id = ?1 AND kind = ?2 AND ref_id = ?3",
                params![deployment_id.to_string(), kind, ref_id],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Enumerate every registered non-memory referrer for one deployment.
    pub fn deployment_referrers(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Vec<DeploymentReferrer>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT deployment_id, kind, ref_id, created_at_ms
                     FROM deployment_referrers WHERE deployment_id = ?1
                     ORDER BY kind, ref_id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([deployment_id.to_string()], |row| {
                    let id: String = row.get(0)?;
                    Ok(DeploymentReferrer {
                        deployment_id: DeploymentId::from_str(&id)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        kind: row.get(1)?,
                        ref_id: row.get(2)?,
                        created_at_ms: row.get(3)?,
                    })
                })
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Fence a non-active deployment in `SQLite` before waiting on in-memory pins.
    pub fn begin_deployment_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            if worker.active_deployment_id == Some(deployment_id) {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentActive,
                    "active deployment cannot be deleted",
                ));
            }
            let referenced: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deployment_referrers WHERE deployment_id = ?1)",
                    [deployment_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if referenced {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentReferenced,
                    "deployment still has registered referrers",
                ));
            }
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_deployments WHERE id = ?1 AND worker_id = ?2",
                    params![deployment_id.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            if state.as_deref() == Some("deleting") {
                return Ok(());
            }
            let changed = tx
                .execute(
                    "UPDATE worker_deployments SET state = 'deleting'
                 WHERE id = ?1 AND worker_id = ?2 AND state IN ('ready', 'rejected')",
                    params![deployment_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(deployment_not_found());
            }
            Ok(())
        })
    }

    /// Finish a deleting deployment after its process-local pins drained.
    pub fn finalize_deployment_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            read_worker_tx(tx, account_id, worker_id)?;
            let deleting: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM worker_deployments
                        WHERE id = ?1 AND worker_id = ?2 AND state = 'deleting'
                    )",
                    params![deployment_id.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if !deleting {
                return Err(deployment_not_found());
            }
            let referenced: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deployment_referrers WHERE deployment_id = ?1)",
                    [deployment_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if referenced {
                return Err(PlatformError::new(
                    ErrorCode::DeploymentReferenced,
                    "deployment acquired a registered referrer while deleting",
                ));
            }
            tx.execute(
                "DELETE FROM deployment_cron_declarations WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM deployment_cron_configs WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM deployment_queue_consumers WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM queue_producer_bindings WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM workflow_bindings WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM deployment_bindings WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM deployment_vars WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM deployment_secrets WHERE deployment_id = ?1",
                [deployment_id.to_string()],
            )
            .map_err(|_| db_error())?;
            crate::assets::delete_deployment_assets(tx, deployment_id)?;
            let changed = tx
                .execute(
                    "UPDATE worker_deployments SET state = 'tombstoned', deleted_at_ms = ?1
                 WHERE id = ?2 AND worker_id = ?3 AND state = 'deleting'",
                    params![now_ms, deployment_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(deployment_not_found());
            }
            audit(
                tx,
                account_id,
                "deployment.delete",
                "deployment",
                &deployment_id.to_string(),
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )?;
            Ok(())
        })
    }

    /// Tombstone synchronously when the caller has already proven no pins exist.
    pub fn tombstone_deployment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.begin_deployment_delete(account_id, worker_id, deployment_id)?;
        self.finalize_deployment_delete(account_id, worker_id, deployment_id, request_id, now_ms)
    }

    /// List crash-recovery candidates left in `deleting`.
    pub fn deleting_deployments(&self) -> Result<Vec<DeploymentId>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM worker_deployments WHERE state = 'deleting' ORDER BY id")
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    DeploymentId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)
                })
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Re-enter a bounded batch of committed `deleting` rows after process restart.
    pub fn recover_deleting_deployments(
        &self,
        request_id: RequestId,
        now_ms: i64,
        limit: u32,
    ) -> Result<u32, PlatformError> {
        if limit == 0 || limit > 10_000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "delete recovery batch is invalid",
            ));
        }
        self.db.with_immediate(|tx| {
            let candidates = {
                let mut stmt = tx
                    .prepare(
                        "SELECT d.id, d.worker_id, w.account_id
                         FROM worker_deployments d JOIN workers w ON w.id = d.worker_id
                         WHERE d.state = 'deleting'
                           AND NOT EXISTS (
                             SELECT 1 FROM deployment_referrers r WHERE r.deployment_id = d.id
                           )
                         ORDER BY d.id LIMIT ?1",
                    )
                    .map_err(|_| db_error())?;
                let rows = stmt
                    .query_map([i64::from(limit)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(|_| db_error())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|_| db_error())?);
                }
                out
            };
            let mut recovered = 0_u32;
            for (deployment, _worker, account) in candidates {
                tx.execute(
                    "DELETE FROM deployment_cron_declarations WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM deployment_cron_configs WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM deployment_queue_consumers WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM queue_producer_bindings WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM workflow_bindings WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM deployment_bindings WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM deployment_vars WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM deployment_secrets WHERE deployment_id = ?1",
                    [&deployment],
                )
                .map_err(|_| db_error())?;
                let deployment_id = DeploymentId::from_str(&deployment).map_err(|_| invariant())?;
                crate::assets::delete_deployment_assets(tx, deployment_id)?;
                let changed = tx
                    .execute(
                        "UPDATE worker_deployments
                         SET state = 'tombstoned', deleted_at_ms = ?1
                         WHERE id = ?2 AND state = 'deleting'",
                        params![now_ms, deployment],
                    )
                    .map_err(|_| db_error())?;
                if changed == 1 {
                    let account_id = AccountId::from_str(&account).map_err(|_| invariant())?;
                    audit(
                        tx,
                        account_id,
                        "deployment.delete.recover",
                        "deployment",
                        &deployment,
                        request_id,
                        br#"{"state":"tombstoned"}"#,
                        now_ms,
                    )?;
                    recovered = recovered.saturating_add(1);
                }
            }
            Ok(recovered)
        })
    }

    /// Remove expired idempotency rows and their registered deployment refs atomically.
    pub fn prune_expired_idempotency(&self, now_ms: i64, limit: u32) -> Result<u32, PlatformError> {
        if limit == 0 || limit > 10_000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "idempotency prune batch is invalid",
            ));
        }
        self.db.with_immediate(|tx| {
            let expired = {
                let mut stmt = tx
                    .prepare(
                        "SELECT account_id, scope, idempotency_key, deployment_id
                         FROM control_idempotency
                         WHERE expires_at_ms <= ?1 ORDER BY expires_at_ms LIMIT ?2",
                    )
                    .map_err(|_| db_error())?;
                let rows = stmt
                    .query_map(params![now_ms, i64::from(limit)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .map_err(|_| db_error())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|_| db_error())?);
                }
                out
            };
            let mut pruned = 0_u32;
            for (account, scope, key, deployment) in expired {
                if let Some(deployment) = deployment {
                    let account_id = AccountId::from_str(&account).map_err(|_| invariant())?;
                    tx.execute(
                        "DELETE FROM deployment_referrers
                         WHERE deployment_id = ?1 AND kind = 'control_idempotency' AND ref_id = ?2",
                        params![deployment, idempotency_ref_id(account_id, &scope, &key)],
                    )
                    .map_err(|_| db_error())?;
                }
                pruned = pruned.saturating_add(
                    u32::try_from(
                        tx.execute(
                            "DELETE FROM control_idempotency
                             WHERE account_id = ?1 AND scope = ?2 AND idempotency_key = ?3
                               AND expires_at_ms <= ?4",
                            params![account, scope, key, now_ms],
                        )
                        .map_err(|_| db_error())?,
                    )
                    .unwrap_or(0),
                );
            }
            Ok(pruned)
        })
    }

    /// Select a bounded retention batch without mutating any row.
    pub fn retention_candidates(
        &self,
        now_ms: i64,
        min_age_ms: u64,
        retain_ready: u32,
        retain_rejected: u32,
        limit: u32,
    ) -> Result<Vec<RetentionCandidate>, PlatformError> {
        if retain_ready == 0
            || retain_rejected == 0
            || limit == 0
            || limit > 10_000
            || min_age_ms > i64::MAX as u64
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "deployment retention policy is invalid",
            ));
        }
        let cutoff = now_ms.saturating_sub(i64::try_from(min_age_ms).map_err(|_| invariant())?);
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT d.id, d.worker_id, w.account_id
                     FROM worker_deployments d JOIN workers w ON w.id = d.worker_id
                     WHERE d.state IN ('ready', 'rejected')
                       AND d.created_at_ms <= ?1
                       AND (w.active_deployment_id IS NULL OR w.active_deployment_id != d.id)
                       AND NOT EXISTS (
                         SELECT 1 FROM deployment_referrers r WHERE r.deployment_id = d.id
                       )
                       AND (
                         (d.state = 'ready' AND (
                           SELECT count(*) FROM worker_deployments newer
                           WHERE newer.worker_id = d.worker_id AND newer.state = 'ready'
                             AND newer.version_number > d.version_number
                         ) >= ?2)
                         OR
                         (d.state = 'rejected' AND (
                           SELECT count(*) FROM worker_deployments newer
                           WHERE newer.worker_id = d.worker_id AND newer.state = 'rejected'
                             AND newer.version_number > d.version_number
                         ) >= ?3)
                       )
                     ORDER BY d.created_at_ms, d.id LIMIT ?4",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map(
                    params![
                        cutoff,
                        i64::from(retain_ready),
                        i64::from(retain_rejected),
                        i64::from(limit)
                    ],
                    |row| {
                        let deployment: String = row.get(0)?;
                        let worker: String = row.get(1)?;
                        let account: String = row.get(2)?;
                        Ok(RetentionCandidate {
                            account_id: AccountId::from_str(&account)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            worker_id: WorkerId::from_str(&worker)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            deployment_id: DeploymentId::from_str(&deployment)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    },
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Return all artifact references still retained by non-tombstoned deployments.
    pub fn referenced_artifacts(&self) -> Result<Vec<([u8; 32], u64)>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT r.sha256, r.size
                     FROM deployment_object_refs r
                     JOIN worker_deployments d ON d.id = r.deployment_id
                     WHERE d.state != 'tombstoned'
                     UNION
                     SELECT DISTINCT o.sha256, o.size
                     FROM deployment_upload_objects o
                     JOIN deployment_uploads u ON u.id = o.session_id
                     WHERE o.verified = 1 AND u.status IN ('open', 'finalizing')",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([], |row| {
                    let digest: Vec<u8> = row.get(0)?;
                    let size: i64 = row.get(1)?;
                    Ok((digest, size))
                })
                .map_err(|_| db_error())?;
            let mut out = Vec::new();
            for row in rows {
                let (digest, size) = row.map_err(|_| db_error())?;
                out.push((
                    array32(&digest).map_err(|_| invariant())?,
                    u64::try_from(size).map_err(|_| db_error())?,
                ));
            }
            Ok(out)
        })
    }
}

pub(crate) fn validate_worker_name(name: &str) -> Result<(), PlatformError> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 63
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || bytes
            .iter()
            .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && *b != b'-')
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "Worker name must be a lowercase ASCII slug",
        ));
    }
    Ok(())
}

pub(crate) fn validate_referrer(kind: &str, ref_id: &str) -> Result<(), PlatformError> {
    let valid = |value: &str, max: usize| {
        !value.is_empty()
            && value.len() <= max
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
            })
    };
    if !valid(kind, 64) || !valid(ref_id, 256) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "deployment referrer token is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/deployment-referrer/v1\0");
    hasher.update(account_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn validate_exact_route(
    hostname: &str,
    path: &str,
    entrypoint: Option<&str>,
) -> Result<(), PlatformError> {
    if hostname.is_empty()
        || hostname.len() > 253
        || hostname.bytes().any(|byte| {
            !byte.is_ascii_lowercase()
                && !byte.is_ascii_digit()
                && !matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
        || path.is_empty()
        || path.len() > 2048
        || !path.starts_with('/')
        || path.contains(['?', '#', '\0'])
        || entrypoint.is_some_and(|value| {
            value.is_empty()
                || value.len() > 128
                || value
                    .bytes()
                    .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
        })
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "exact route input is invalid",
        ));
    }
    Ok(())
}

fn require_account(tx: &Transaction<'_>, account_id: AccountId) -> Result<(), PlatformError> {
    let found: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1 AND deleted_at_ms IS NULL)",
            [account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if found {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::AccountNotFound,
            "account was not found",
        ))
    }
}

fn require_live_worker(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    read_worker_tx(tx, account_id, worker_id).and_then(|worker| {
        if worker.deleted_at_ms.is_some() {
            Err(PlatformError::new(
                ErrorCode::WorkerDeleted,
                "Worker is tombstoned",
            ))
        } else {
            Ok(worker)
        }
    })
}

fn read_worker_tx(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, name, active_deployment_id, do_storage_id,
                route_generation, created_at_ms, updated_at_ms, deleted_at_ms
         FROM workers WHERE id = ?1 AND account_id = ?2",
        params![worker_id.to_string(), account_id.to_string()],
        map_worker,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(worker_not_found)
}

fn read_vars(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<BTreeMap<String, Vec<u8>>, PlatformError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, value_json FROM deployment_vars WHERE deployment_id = ?1 ORDER BY name",
        )
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([deployment_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|_| db_error())?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (name, value) = row.map_err(|_| db_error())?;
        out.insert(name, value);
    }
    Ok(out)
}

fn read_secrets(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<BTreeMap<String, StoredDeploymentSecret>, PlatformError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, revision_id, key_id, algorithm, nonce, ciphertext
         FROM deployment_secrets WHERE deployment_id = ?1 ORDER BY name",
        )
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([deployment_id.to_string()], |row| {
            let name: String = row.get(0)?;
            Ok((
                name.clone(),
                StoredDeploymentSecret {
                    name,
                    revision_id: row.get(1)?,
                    envelope: SecretEnvelope {
                        version: 1,
                        key_id: row.get(2)?,
                        algorithm: row.get(3)?,
                        nonce: row.get(4)?,
                        ciphertext: row.get(5)?,
                    },
                },
            ))
        })
        .map_err(|_| db_error())?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (name, value) = row.map_err(|_| db_error())?;
        out.insert(name, value);
    }
    Ok(out)
}

fn map_worker(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRecord> {
    let id: String = row.get(0)?;
    let account: String = row.get(1)?;
    let active: Option<String> = row.get(3)?;
    let generation: i64 = row.get(5)?;
    Ok(WorkerRecord {
        id: WorkerId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(2)?,
        active_deployment_id: active
            .map(|value| DeploymentId::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        do_storage_id: row.get(4)?,
        route_generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
        deleted_at_ms: row.get(8)?,
    })
}

fn map_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRecord> {
    let id: String = row.get(0)?;
    let worker: String = row.get(1)?;
    let version: i64 = row.get(2)?;
    let content_kind: String = row.get(3)?;
    let state: String = row.get(4)?;
    let artifact: Option<Vec<u8>> = row.get(5)?;
    let artifact_size: Option<i64> = row.get(6)?;
    let artifact_schema: Option<i64> = row.get(7)?;
    let flags: Vec<u8> = row.get(10)?;
    let limits: Vec<u8> = row.get(11)?;
    let descriptor: Vec<u8> = row.get(12)?;
    let loader_schema: i64 = row.get(13)?;
    Ok(DeploymentRecord {
        id: DeploymentId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_number: u64::try_from(version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        content_kind: DeploymentContentKind::parse(&content_kind)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: DeploymentState::parse(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        artifact_sha256: artifact.as_deref().map(array32).transpose()?,
        artifact_size: artifact_size
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        artifact_schema_version: artifact_schema
            .map(u32::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        main_module: row.get(8)?,
        compatibility_date: row.get(9)?,
        compatibility_flags: serde_json::from_slice(&flags)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        limits: serde_json::from_slice(&limits).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_code_sha256: array32(&descriptor)?,
        loader_schema_version: u32::try_from(loader_schema)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(14)?,
        ready_at_ms: row.get(15)?,
        rejected_at_ms: row.get(16)?,
        rejection_code: row.get(17)?,
        deleted_at_ms: row.get(18)?,
    })
}

fn map_route(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouteRecord> {
    let account: String = row.get(1)?;
    let worker: String = row.get(2)?;
    let kind: String = row.get(3)?;
    let generation: i64 = row.get(7)?;
    Ok(RouteRecord {
        id: row.get(0)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: RouteKind::parse(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        hostname_ascii: row.get(4)?,
        path_prefix: row.get(5)?,
        entrypoint: row.get(6)?,
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|_| db_error())?);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn audit(
    tx: &Transaction<'_>,
    account_id: AccountId,
    action: &str,
    target_type: &str,
    target_id: &str,
    request_id: RequestId,
    details: &[u8],
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO control_audit_events
         (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            account_id.to_string(),
            action,
            target_type,
            target_id,
            request_id.to_string(),
            details,
            now_ms
        ],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

pub(crate) fn array32(bytes: &[u8]) -> rusqlite::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn worker_not_found() -> PlatformError {
    PlatformError::new(ErrorCode::WorkerNotFound, "Worker was not found")
}

pub(crate) fn deployment_not_found() -> PlatformError {
    PlatformError::new(ErrorCode::DeploymentNotFound, "deployment was not found")
}

pub(crate) fn route_not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::RouteNotFound,
        "no active route matched the request",
    )
}

pub(crate) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "persisted deployment invariant failed",
    )
}

pub(crate) fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}
