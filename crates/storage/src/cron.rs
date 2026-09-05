//! Immutable Cron declarations and live activation authority.

use crate::ControlDb;
use open_compute_core::{
    AccountId, CronActivationId, ErrorCode, PlatformError, VersionId, WorkerId,
};
use rusqlite::{Transaction, params};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::str::FromStr;

/// Cron parser/canonicalization contract shipped by P2.3.
pub const CRON_PARSER_VERSION: u32 = 1;

/// Frozen Cron config inserted with a staging version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewCronConfig {
    /// Capability version.
    pub capability_version: u32,
    /// Canonical config digest.
    pub descriptor_sha256: [u8; 32],
    /// Exact validated declarations for replacement mode.
    pub declarations: Vec<NewCronDeclaration>,
}

/// One exact validated Cron expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewCronDeclaration {
    /// Platform-generated declaration identity.
    pub id: CronActivationId,
    /// Tenant-visible exact expression.
    pub expression: String,
    /// Parser-normalized expression digest.
    pub expression_sha256: [u8; 32],
    /// Parser contract version.
    pub parser_version: u32,
    /// Whether this expression invokes the Worker's scheduled handler.
    pub scheduled_handler: bool,
    /// Workflow bindings directly triggered by this expression.
    pub workflow_bindings: Vec<String>,
}

/// Immutable version Cron config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronVersionConfig {
    /// Owning version.
    pub version_id: VersionId,
    /// Capability version.
    pub capability_version: u32,
    /// Config digest.
    pub descriptor_sha256: [u8; 32],
    /// Creation time.
    pub created_at_ms: i64,
    /// Ordered exact expressions.
    pub declarations: Vec<CronDeclaration>,
}

impl CronVersionConfig {
    /// Verify the complete canonical Day1 declaration set and its descriptor.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.capability_version != 1
            || self.declarations.len() > 100
            || self
                .declarations
                .windows(2)
                .any(|pair| pair[0].expression >= pair[1].expression)
            || self.declarations.iter().any(|declaration| {
                declaration.version_id != self.version_id
                    || !valid_declaration(
                        &declaration.expression,
                        &declaration.expression_sha256,
                        declaration.parser_version,
                        declaration.scheduled_handler,
                        &declaration.workflow_bindings,
                    )
            })
        {
            return Err(invariant());
        }
        let descriptor = serde_json::json!({
            "capabilityVersion": self.capability_version,
            "declarations": self.declarations.iter().map(|declaration| serde_json::json!({
                "expression": declaration.expression,
                "expressionSha256": hex::encode(declaration.expression_sha256),
                "parserVersion": declaration.parser_version,
                "scheduledHandler": declaration.scheduled_handler,
                "workflowBindings": declaration.workflow_bindings,
            })).collect::<Vec<_>>(),
        });
        let actual: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&descriptor).map_err(|_| invariant())?).into();
        if actual != self.descriptor_sha256 {
            return Err(invariant());
        }
        Ok(())
    }
}

/// Immutable Cron expression declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronDeclaration {
    /// Declaration identity.
    pub id: CronActivationId,
    /// Owning version.
    pub version_id: VersionId,
    /// Tenant-visible exact expression.
    pub expression: String,
    /// Parser-normalized digest.
    #[serde(skip)]
    pub expression_sha256: [u8; 32],
    /// Parser contract version.
    pub parser_version: u32,
    /// Whether this expression invokes the Worker's scheduled handler.
    pub scheduled_handler: bool,
    /// Workflow bindings directly triggered by this expression.
    pub workflow_bindings: Vec<String>,
    /// Creation time.
    pub created_at_ms: i64,
}

/// Live Cron activation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CronActivationState {
    /// Scheduler projection is staged but not accepting slots.
    Staging,
    /// New logical slots may be projected.
    Active,
    /// Old activation drains without new slots.
    Retiring,
    /// Immutable retired activation.
    Tombstoned,
}

impl FromStr for CronActivationState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "staging" => Ok(Self::Staging),
            "active" => Ok(Self::Active),
            "retiring" => Ok(Self::Retiring),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Live Cron activation row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronActivationRecord {
    /// Activation identity.
    pub id: CronActivationId,
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen target version.
    pub version_id: VersionId,
    /// Exact expression.
    pub expression: String,
    /// Parser-normalized digest.
    #[serde(skip)]
    pub expression_sha256: [u8; 32],
    /// Parser contract version.
    pub parser_version: u32,
    /// Whether this expression invokes the Worker's scheduled handler.
    pub scheduled_handler: bool,
    /// Workflow bindings directly triggered by this expression.
    pub workflow_bindings: Vec<String>,
    /// Monotonic set generation.
    pub activation_generation: u64,
    /// Lifecycle state.
    pub state: CronActivationState,
    /// Stable availability spelling.
    pub availability: String,
    /// Stable reason when unavailable.
    pub availability_code: Option<String>,
    /// Creation time.
    pub created_at_ms: i64,
    /// Last mutation time.
    pub updated_at_ms: i64,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

/// Control repository for version Cron declarations and live activations.
#[derive(Clone, Copy, Debug)]
pub struct CronRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> CronRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// List a bounded global operator view of non-tombstoned Cron activations.
    pub fn list_live(&self, limit: u32) -> Result<Vec<CronActivationRecord>, PlatformError> {
        if limit == 0 {
            return Err(invariant());
        }
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, account_id, worker_id, version_id, expression,
                            expression_sha256, parser_version, scheduled_handler,
                            workflow_bindings_json, activation_generation,
                            state, availability, availability_code, created_at_ms,
                            updated_at_ms, deleted_at_ms
                     FROM cron_activations WHERE state != 'tombstoned'
                     ORDER BY account_id, worker_id, activation_generation, expression, id
                     LIMIT ?1",
                )
                .map_err(|_| invariant())?;
            statement
                .query_map([i64::from(limit)], map_activation)
                .map_err(|_| invariant())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invariant())
        })
    }

    /// Read one version's immutable Cron config.
    pub fn version_config(
        &self,
        version_id: VersionId,
    ) -> Result<CronVersionConfig, PlatformError> {
        self.db.with_read(|connection| {
            let (capability_version, descriptor, created_at_ms): (i64, Vec<u8>, i64) = connection
                .query_row(
                    "SELECT capability_version, descriptor_sha256, created_at_ms
                     FROM version_cron_configs WHERE version_id = ?1",
                    [version_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|_| invariant())?;
            let mut statement = connection
                .prepare(
                    "SELECT id, version_id, expression, expression_sha256,
                            parser_version, scheduled_handler, workflow_bindings_json, created_at_ms
                     FROM version_cron_declarations WHERE version_id = ?1
                     ORDER BY expression, id",
                )
                .map_err(|_| invariant())?;
            let declarations = statement
                .query_map([version_id.to_string()], map_declaration)
                .map_err(|_| invariant())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invariant())?;
            let config = CronVersionConfig {
                version_id,
                capability_version: u32::try_from(capability_version).map_err(|_| invariant())?,
                descriptor_sha256: digest(descriptor).map_err(|_| invariant())?,
                created_at_ms,
                declarations,
            };
            config.validate()?;
            Ok(config)
        })
    }

    /// Highest persisted activation generation, including retired history; zero before first use.
    pub fn maximum_generation(&self, worker_id: WorkerId) -> Result<u64, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(activation_generation), 0)
                 FROM cron_activations WHERE worker_id = ?1",
                    [worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())
        })
    }

    /// List non-tombstoned activations for one Worker.
    pub fn live_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<CronActivationRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, account_id, worker_id, version_id, expression,
                            expression_sha256, parser_version, scheduled_handler,
                            workflow_bindings_json, activation_generation,
                            state, availability, availability_code, created_at_ms,
                            updated_at_ms, deleted_at_ms
                     FROM cron_activations WHERE worker_id = ?1 AND state != 'tombstoned'
                     ORDER BY activation_generation, expression, id",
                )
                .map_err(|_| invariant())?;
            statement
                .query_map([worker_id.to_string()], map_activation)
                .map_err(|_| invariant())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invariant())
        })
    }

    /// Read one exact activation used by a scheduler claim.
    pub fn activation(&self, id: CronActivationId) -> Result<CronActivationRecord, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, account_id, worker_id, version_id, expression,
                            expression_sha256, parser_version, scheduled_handler,
                            workflow_bindings_json, activation_generation, state, availability,
                            availability_code, created_at_ms, updated_at_ms, deleted_at_ms
                     FROM cron_activations WHERE id = ?1",
                    [id.to_string()],
                    map_activation,
                )
                .map_err(|_| invariant())
        })
    }

    /// Stage an exact activation set for a ready version.
    pub fn stage_activations(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        generation: u64,
        declarations: &[CronDeclaration],
        now_ms: i64,
    ) -> Result<Vec<CronActivationRecord>, PlatformError> {
        self.db.with_immediate(|tx| {
            let mut activations = Vec::with_capacity(declarations.len());
            for declaration in declarations {
                let id = CronActivationId::generate();
                tx.execute(
                    "INSERT INTO cron_activations
                     (id, account_id, worker_id, version_id, expression,
                      expression_sha256, parser_version, scheduled_handler,
                      workflow_bindings_json, activation_generation,
                      state, availability, availability_code, created_at_ms,
                      updated_at_ms, deleted_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                             'staging', 'degraded', 'CRON_PROJECTION_PENDING', ?11, ?11, NULL)",
                    params![
                        id.to_string(),
                        account_id.to_string(),
                        worker_id.to_string(),
                        version_id.to_string(),
                        declaration.expression,
                        declaration.expression_sha256.as_slice(),
                        i64::from(declaration.parser_version),
                        declaration.scheduled_handler,
                        serde_json::to_vec(&declaration.workflow_bindings)
                            .map_err(|_| invariant())?,
                        as_i64(generation)?,
                        now_ms,
                    ],
                )
                .map_err(|_| invariant())?;
                activations.push(read_activation_tx(tx, id)?);
            }
            Ok(activations)
        })
    }

    /// Mark every exact staged activation accepting after scheduler projection commit.
    pub fn activate_generation(
        &self,
        worker_id: WorkerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE cron_activations SET state = 'active', availability = 'healthy',
                            availability_code = NULL, updated_at_ms = ?1
                     WHERE worker_id = ?2 AND activation_generation = ?3 AND state = 'staging'",
                    params![now_ms, worker_id.to_string(), as_i64(generation)?],
                )
                .map_err(|_| invariant())?;
            u64::try_from(changed).map_err(|_| invariant())
        })
    }

    /// Fence active old-generation activations against new logical slots.
    pub fn retire_before(
        &self,
        worker_id: WorkerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE cron_activations SET state = 'retiring', availability = 'degraded',
                            availability_code = 'CRON_DRAINING', updated_at_ms = ?1
                     WHERE worker_id = ?2 AND activation_generation < ?3
                       AND state IN ('staging', 'active')",
                    params![now_ms, worker_id.to_string(), as_i64(generation)?],
                )
                .map_err(|_| invariant())?;
            u64::try_from(changed).map_err(|_| invariant())
        })
    }

    /// Tombstone one fully drained activation and release its version referrer.
    pub fn finish_retire(
        &self,
        id: CronActivationId,
        generation: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE cron_activations SET state = 'tombstoned', availability = 'unavailable',
                            availability_code = 'CRON_RETIRED', updated_at_ms = ?1,
                            deleted_at_ms = ?1
                     WHERE id = ?2 AND activation_generation = ?3 AND state = 'retiring'",
                    params![now_ms, id.to_string(), as_i64(generation)?],
                )
                .map_err(|_| invariant())?;
            Ok(changed == 1)
        })
    }
}

pub(crate) fn insert_staging_config(
    tx: &Transaction<'_>,
    version_id: VersionId,
    config: &NewCronConfig,
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO version_cron_configs
         (version_id, capability_version, descriptor_sha256, created_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            version_id.to_string(),
            i64::from(config.capability_version),
            config.descriptor_sha256.as_slice(),
            now_ms,
        ],
    )
    .map_err(|_| invariant())?;
    for declaration in &config.declarations {
        tx.execute(
            "INSERT INTO version_cron_declarations
             (id, version_id, expression, expression_sha256, parser_version,
              scheduled_handler, workflow_bindings_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                declaration.id.to_string(),
                version_id.to_string(),
                declaration.expression,
                declaration.expression_sha256.as_slice(),
                i64::from(declaration.parser_version),
                declaration.scheduled_handler,
                serde_json::to_vec(&declaration.workflow_bindings).map_err(|_| invariant())?,
                now_ms,
            ],
        )
        .map_err(|_| invariant())?;
    }
    Ok(())
}

fn read_activation_tx(
    tx: &Transaction<'_>,
    id: CronActivationId,
) -> Result<CronActivationRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, worker_id, version_id, expression,
                expression_sha256, parser_version, scheduled_handler, workflow_bindings_json,
                activation_generation, state,
                availability, availability_code, created_at_ms, updated_at_ms, deleted_at_ms
         FROM cron_activations WHERE id = ?1",
        [id.to_string()],
        map_activation,
    )
    .map_err(|_| invariant())
}

fn map_declaration(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronDeclaration> {
    let declaration = CronDeclaration {
        id: parse(&row.get::<_, String>(0)?)?,
        version_id: parse(&row.get::<_, String>(1)?)?,
        expression: row.get(2)?,
        expression_sha256: digest(row.get(3)?)?,
        parser_version: unsigned(row.get(4)?)?,
        scheduled_handler: row.get(5)?,
        workflow_bindings: serde_json::from_slice(&row.get::<_, Vec<u8>>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(7)?,
    };
    if !valid_declaration(
        &declaration.expression,
        &declaration.expression_sha256,
        declaration.parser_version,
        declaration.scheduled_handler,
        &declaration.workflow_bindings,
    ) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(declaration)
}

fn map_activation(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronActivationRecord> {
    let activation = CronActivationRecord {
        id: parse(&row.get::<_, String>(0)?)?,
        account_id: parse(&row.get::<_, String>(1)?)?,
        worker_id: parse(&row.get::<_, String>(2)?)?,
        version_id: parse(&row.get::<_, String>(3)?)?,
        expression: row.get(4)?,
        expression_sha256: digest(row.get(5)?)?,
        parser_version: unsigned(row.get(6)?)?,
        scheduled_handler: row.get(7)?,
        workflow_bindings: serde_json::from_slice(&row.get::<_, Vec<u8>>(8)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        activation_generation: unsigned(row.get(9)?)?,
        state: row
            .get::<_, String>(10)?
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: row.get(11)?,
        availability_code: row.get(12)?,
        created_at_ms: row.get(13)?,
        updated_at_ms: row.get(14)?,
        deleted_at_ms: row.get(15)?,
    };
    if !valid_declaration(
        &activation.expression,
        &activation.expression_sha256,
        activation.parser_version,
        activation.scheduled_handler,
        &activation.workflow_bindings,
    ) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(activation)
}

fn valid_declaration(
    expression: &str,
    expression_sha256: &[u8; 32],
    parser_version: u32,
    scheduled_handler: bool,
    workflow_bindings: &[String],
) -> bool {
    let Ok(parsed) = open_compute_core::CronSchedule::parse(expression) else {
        return false;
    };
    parser_version == CRON_PARSER_VERSION
        && Sha256::digest(parsed.normalized().as_bytes()).as_slice() == expression_sha256
        && (scheduled_handler || !workflow_bindings.is_empty())
        && workflow_bindings.len() <= 100
        && !workflow_bindings.iter().any(|name| {
            let bytes = name.as_bytes();
            bytes.is_empty()
                || bytes.len() > 64
                || name.starts_with("__")
                || name.starts_with("OPEN_COMPUTE_")
                || bytes[0].is_ascii_digit()
                || !bytes
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        })
        && !workflow_bindings.windows(2).any(|pair| pair[0] >= pair[1])
}

fn parse<T: FromStr>(value: &str) -> rusqlite::Result<T> {
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn unsigned<T: TryFrom<i64>>(value: i64) -> rusqlite::Result<T> {
    T::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn digest(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| invariant())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::CronProjectionPending,
        "Cron control authority invariant failed",
    )
}
