//! Immutable Cron declarations and live activation authority.

use crate::control_db::ControlDb;
use open_compute_core::{
    CronActivationId, ErrorCode, InstanceId, PlatformError, VersionId, WorkerId,
};
use rusqlite::{Transaction, params};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// Normalization identity version; the a/b scheduling fix preserves version-1 declaration hashes.
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

/// Live Cron activation row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronActivationRecord {
    /// Activation identity.
    pub id: CronActivationId,
    /// Owning instance.
    pub instance_id: InstanceId,
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
    /// Creation time.
    pub created_at_ms: i64,
    /// Last mutation time.
    pub updated_at_ms: i64,
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

    /// List a bounded global operator view of live Cron activations.
    pub fn list_live(&self, limit: u32) -> Result<Vec<CronActivationRecord>, PlatformError> {
        if limit == 0 {
            return Err(invariant());
        }
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, (SELECT instance_id FROM instance_identity), worker_id, version_id, expression,
                            expression_sha256, parser_version, scheduled_handler,
                            workflow_bindings_json,
                            created_at_ms,
                            updated_at_ms
                     FROM cron_activations
                     ORDER BY worker_id, expression, id
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
                    [version_id],
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
                .query_map([version_id], map_declaration)
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

    /// List live activations for one Worker.
    pub fn live_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<CronActivationRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, (SELECT instance_id FROM instance_identity), worker_id, version_id, expression,
                            expression_sha256, parser_version, scheduled_handler,
                            workflow_bindings_json,
                            created_at_ms,
                            updated_at_ms
                     FROM cron_activations WHERE worker_id = ?1
                     ORDER BY expression, id",
                )
                .map_err(|_| invariant())?;
            statement
                .query_map([worker_id], map_activation)
                .map_err(|_| invariant())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invariant())
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
            version_id,
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
                declaration.id,
                version_id,
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

fn map_declaration(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronDeclaration> {
    let declaration = CronDeclaration {
        id: row.get(0)?,
        version_id: row.get(1)?,
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
        id: row.get(0)?,
        instance_id: row.get(1)?,
        worker_id: row.get(2)?,
        version_id: row.get(3)?,
        expression: row.get(4)?,
        expression_sha256: digest(row.get(5)?)?,
        parser_version: unsigned(row.get(6)?)?,
        scheduled_handler: row.get(7)?,
        workflow_bindings: serde_json::from_slice(&row.get::<_, Vec<u8>>(8)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
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

fn unsigned<T: TryFrom<i64>>(value: i64) -> rusqlite::Result<T> {
    T::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn digest(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::CronProjectionPending,
        "Cron control authority invariant failed",
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CronActivationState {
    Active,
}
impl rusqlite::types::ToSql for CronActivationState {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(match self {
            Self::Active => "active",
        }
        .into())
    }
}
impl rusqlite::types::FromSql for CronActivationState {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "active" => Ok(Self::Active),
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_one_stored_step_declaration_keeps_its_identity() {
        assert_eq!(CRON_PARSER_VERSION, 1);
        let version_id = VersionId::generate();
        // Frozen version-1 hashes use the original expression and normalized spelling, not fire times.
        let mut config = CronVersionConfig {
            version_id,
            capability_version: 1,
            descriptor_sha256: hex::decode(
                "4cefccc767aca9860006e765ca2d69d9c1c0d1d4ab29b77674d04057fcb9127d",
            )
            .unwrap()
            .try_into()
            .unwrap(),
            created_at_ms: 0,
            declarations: vec![CronDeclaration {
                id: CronActivationId::generate(),
                version_id,
                expression: "5/5\t * * * *".into(),
                expression_sha256: hex::decode(
                    "9eddee43ec656b021f8c2441a5b4e345e23f8dc4810eeb667fb7c683239eb556",
                )
                .unwrap()
                .try_into()
                .unwrap(),
                parser_version: 1,
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
                created_at_ms: 0,
            }],
        };
        config.validate().unwrap();
        assert_eq!(
            open_compute_core::CronSchedule::parse(&config.declarations[0].expression)
                .unwrap()
                .next_after_ms(5 * 60_000)
                .unwrap(),
            10 * 60_000
        );
        config.declarations[0].parser_version = 2;
        assert_eq!(
            config.validate().unwrap_err().code(),
            ErrorCode::CronProjectionPending
        );
        config.declarations[0].parser_version = 1;
        config.declarations[0].expression_sha256[0] ^= 1;
        assert_eq!(
            config.validate().unwrap_err().code(),
            ErrorCode::CronProjectionPending
        );
    }
}
