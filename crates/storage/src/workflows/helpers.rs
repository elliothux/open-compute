use super::*;
use rand::TryRngCore as _;
use rusqlite::types::Type;

pub(super) fn error(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow operation failed")
}
pub(crate) fn invariant() -> PlatformError {
    error(ErrorCode::WorkflowInvariantViolation)
}
// `Result::map_err` transfers the driver error; no raw SQL detail escapes this boundary.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn sql_error(err: rusqlite::Error) -> PlatformError {
    let code = match err {
        rusqlite::Error::SqliteFailure(info, _)
            if matches!(
                info.code,
                rusqlite::ErrorCode::DatabaseBusy
                    | rusqlite::ErrorCode::DatabaseLocked
                    | rusqlite::ErrorCode::DiskFull
            ) =>
        {
            ErrorCode::WorkflowRuntimeUnavailable
        }
        _ => ErrorCode::WorkflowInvariantViolation,
    };
    error(code)
}

pub(crate) fn token() -> Result<WorkflowToken, PlatformError> {
    let mut bytes = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| error(ErrorCode::WorkflowRuntimeUnavailable))?;
    Ok(WorkflowToken::from_bytes(bytes))
}

pub(super) fn parse<T: std::str::FromStr>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    row.get::<_, String>(index)?.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(invariant()))
    })
}

pub(super) fn digest(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<[u8; 32]> {
    row.get::<_, Vec<u8>>(index)?.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Blob, Box::new(invariant()))
    })
}

pub(super) fn definition_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowDefinition> {
    Ok(WorkflowDefinition {
        id: parse(row, 0)?,
        account_id: parse(row, 1)?,
        name: row.get(2)?,
        state: parse(row, 3)?,
        availability: parse(row, 4)?,
        availability_code: row.get(5)?,
        lifecycle_generation: row.get(6)?,
        reserved_class_name: row.get(7)?,
        reservation_owner: row.get(8)?,
        reservation_fence: row.get(9)?,
        reservation_state: row
            .get::<_, Option<String>>(10)?
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        reservation_created_definition: row.get(11)?,
        delete_fence: row.get(12)?,
        current_version_id: row
            .get::<_, Option<String>>(13)?
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        created_at_ms: row.get(14)?,
        updated_at_ms: row.get(15)?,
    })
}

pub(super) fn target_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowTarget> {
    Ok(WorkflowTarget {
        account_id: parse(row, 0)?,
        definition_id: parse(row, 1)?,
        definition_name: row.get(2)?,
        workflow_version_id: parse(row, 3)?,
        worker_id: parse(row, 4)?,
        worker_version_id: parse(row, 5)?,
        worker_code_sha256: digest(row, 6)?,
        class_name: row.get(7)?,
        loader_schema_version: row.get(8)?,
        capability_version: row.get(9)?,
        descriptor_sha256: digest(row, 10)?,
    })
}

pub(super) fn version_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowVersion> {
    Ok(WorkflowVersion {
        target: target_row(row)?,
        version_number: row.get(11)?,
        state: VersionState::parse(&row.get::<_, String>(12)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(13)?,
        rejection_code: row.get(14)?,
        reservation_owner: row.get(15)?,
        reservation_fence: row.get(16)?,
    })
}

pub(super) const DEFINITION_SELECT: &str =
    "SELECT id,account_id,name,state,availability,availability_code,
    lifecycle_generation,reserved_class_name,reservation_owner,reservation_fence,reservation_state,
    reservation_created_definition,delete_fence,current_version_id,created_at_ms,updated_at_ms FROM workflow_definitions";

pub(super) fn validate_class_name(class_name: &str) -> Result<(), PlatformError> {
    let bytes = class_name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 128
        || class_name.starts_with("__")
        || !(bytes[0].is_ascii_alphabetic() || matches!(bytes[0], b'_' | b'$'))
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
    {
        return Err(error(ErrorCode::WorkflowVersionNotReady));
    }
    Ok(())
}
pub(super) const VERSION_SELECT: &str = "SELECT f.account_id,f.id,f.name,v.id,v.worker_id,v.worker_version_id,
    v.worker_code_sha256,v.class_name,v.loader_schema_version,v.capability_version,v.descriptor_sha256,
    v.version_number,v.state,v.created_at_ms,v.rejection_code,v.reservation_owner,v.reservation_fence FROM workflow_versions v
    JOIN workflow_definitions f ON f.id=v.definition_id";

pub(crate) fn version_digest(target: &WorkflowTarget) -> Result<[u8; 32], PlatformError> {
    if target.capability_version != 1 {
        return Err(invariant());
    }
    // Display name can change; it is copied only when an instance is created.
    let descriptor = serde_json::json!({"schemaVersion":2,"accountId":target.account_id,"definitionId":target.definition_id,
        "workflowVersionId":target.workflow_version_id,"workerId":target.worker_id,"workerVersionId":target.worker_version_id,
        "workerCodeSha256":hex::encode(target.worker_code_sha256),"className":target.class_name,
        "loaderSchemaVersion":target.loader_schema_version,"capabilityVersion":target.capability_version});
    Ok(Sha256::digest(serde_json::to_vec(&descriptor).map_err(|_| invariant())?).into())
}
