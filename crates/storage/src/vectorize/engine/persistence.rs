//! SQLite record encoding, mutation application, and metadata-filter persistence helpers.

use super::{
    MAX_BATCH_ITEMS, MAX_ID_BYTES, MAX_METADATA_BYTES, MAX_NAMESPACE_BYTES, VectorMutation,
    VectorMutationKind, VectorMutationState, VectorRecord,
};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_search::{FilterExpr, FilterOperator, FilterPredicate, MetadataScalar};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

const MAX_PENDING_MUTATIONS: u64 = 1_024;
const MAX_PENDING_ITEMS: u64 = 100_000;
const RETAIN_APPLIED_MUTATIONS: u64 = 10_000;
type StoredVectorRow = (String, Option<String>, Vec<u8>, Option<Vec<u8>>);

#[derive(Debug)]
pub(super) struct StoredItem {
    pub(super) id: String,
    pub(super) namespace: Option<String>,
    pub(super) values: Option<Vec<u8>>,
    pub(super) metadata: Option<Vec<u8>>,
}

pub(super) fn validate_pending_projection(
    tx: &Transaction<'_>,
    quota_vectors: u64,
    quota_bytes: u64,
) -> Result<(), PlatformError> {
    let failed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM vector_mutations WHERE state = 'failed')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| corrupt())?;
    if failed {
        return Err(frontier_blocked());
    }
    let (mutations, items, payload): (i64, i64, i64) = tx
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(item_count), 0), COALESCE(SUM(payload_bytes), 0)
             FROM vector_mutations WHERE state IN ('queued', 'claimed')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| corrupt())?;
    if u64::try_from(mutations).map_err(|_| corrupt())? > MAX_PENDING_MUTATIONS
        || u64::try_from(items).map_err(|_| corrupt())? > MAX_PENDING_ITEMS
        || u64::try_from(payload).map_err(|_| corrupt())? > quota_bytes
    {
        return Err(limit());
    }

    let mut projected = HashMap::new();
    {
        let mut statement = tx
            .prepare(
                "SELECT vector_id,
                        length(CAST(vector_id AS BLOB))
                          + COALESCE(length(CAST(namespace AS BLOB)), 0)
                          + length(values_f32le)
                          + COALESCE(length(metadata_json), 0)
                 FROM vectors",
            )
            .map_err(|_| corrupt())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|_| corrupt())?;
        for row in rows {
            let (id, bytes) = row.map_err(|_| corrupt())?;
            projected.insert(id, u64::try_from(bytes).map_err(|_| corrupt())?);
        }
    }
    {
        let mut statement = tx
            .prepare(
                "SELECT mutation.kind, item.vector_id,
                        length(CAST(item.vector_id AS BLOB))
                          + COALESCE(length(CAST(item.namespace AS BLOB)), 0)
                          + COALESCE(length(item.values_f32le), 0)
                          + COALESCE(length(item.metadata_json), 0)
                 FROM vector_mutations AS mutation
                 JOIN vector_mutation_items AS item
                   ON item.mutation_id = mutation.mutation_id
                 WHERE mutation.state IN ('queued', 'claimed')
                 ORDER BY mutation.sequence, item.ordinal",
            )
            .map_err(|_| corrupt())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| corrupt())?;
        for row in rows {
            let (kind, id, bytes) = row.map_err(|_| corrupt())?;
            let bytes = u64::try_from(bytes).map_err(|_| corrupt())?;
            match kind.as_str() {
                "insert" => {
                    projected.entry(id).or_insert(bytes);
                }
                "upsert" => {
                    projected.insert(id, bytes);
                }
                "delete" => {
                    projected.remove(&id);
                }
                _ => return Err(corrupt()),
            }
        }
    }
    let projected_bytes = projected.values().try_fold(0_u64, |sum, bytes| {
        sum.checked_add(*bytes).ok_or_else(limit)
    })?;
    if u64::try_from(projected.len()).map_err(|_| limit())? > quota_vectors
        || projected_bytes > quota_bytes
    {
        return Err(limit());
    }
    Ok(())
}

pub(super) fn prune_applied_mutation_payload(
    tx: &Transaction<'_>,
    mutation_id: &str,
    sequence: u64,
) -> Result<(), PlatformError> {
    tx.execute(
        "DELETE FROM vector_mutation_items WHERE mutation_id = ?1",
        [mutation_id],
    )
    .map_err(|_| corrupt())?;
    let retain_from = sequence.saturating_sub(RETAIN_APPLIED_MUTATIONS);
    tx.execute(
        "DELETE FROM vector_mutations WHERE state = 'applied' AND sequence <= ?1",
        [i64::try_from(retain_from).map_err(|_| corrupt())?],
    )
    .map_err(|_| corrupt())?;
    Ok(())
}

pub(super) fn read_mutation_at(
    tx: &Transaction<'_>,
    sequence: i64,
) -> Result<Option<VectorMutation>, PlatformError> {
    tx.query_row(
        "SELECT mutation_id, sequence, kind, state, item_count, error_code
         FROM vector_mutations WHERE sequence = ?1",
        [sequence],
        |row| {
            let kind: String = row.get(2)?;
            let state: String = row.get(3)?;
            Ok(VectorMutation {
                mutation_id: row.get(0)?,
                sequence: row.get(1)?,
                kind: match kind.as_str() {
                    "insert" => VectorMutationKind::Insert,
                    "upsert" => VectorMutationKind::Upsert,
                    "delete" => VectorMutationKind::Delete,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                state: match state.as_str() {
                    "queued" => VectorMutationState::Queued,
                    "claimed" => VectorMutationState::Claimed,
                    "applied" => VectorMutationState::Applied,
                    "failed" => VectorMutationState::Failed,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                item_count: row.get(4)?,
                error_code: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|_| corrupt())
}

pub(super) fn read_items(
    tx: &Transaction<'_>,
    mutation_id: &str,
) -> Result<Vec<StoredItem>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT vector_id, namespace, values_f32le, metadata_json
         FROM vector_mutation_items WHERE mutation_id = ?1 ORDER BY ordinal",
        )
        .map_err(|_| corrupt())?;
    let rows = statement
        .query_map([mutation_id], |row| {
            Ok(StoredItem {
                id: row.get(0)?,
                namespace: row.get(1)?,
                values: row.get(2)?,
                metadata: row.get(3)?,
            })
        })
        .map_err(|_| corrupt())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|_| corrupt())
}

pub(super) fn validate_persisted_items(
    items: &[StoredItem],
    kind: VectorMutationKind,
    dimensions: usize,
) -> Result<(), PlatformError> {
    if items.is_empty() || items.len() > MAX_BATCH_ITEMS {
        return Err(corrupt());
    }
    let mut ids = BTreeSet::new();
    for item in items {
        if !valid_identity(&item.id, MAX_ID_BYTES) || !ids.insert(item.id.as_str()) {
            return Err(corrupt());
        }
        if item
            .namespace
            .as_deref()
            .is_some_and(|value| !valid_identity(value, MAX_NAMESPACE_BYTES))
        {
            return Err(corrupt());
        }
        match (kind, item.values.as_deref()) {
            (VectorMutationKind::Delete, None) => {}
            (VectorMutationKind::Insert | VectorMutationKind::Upsert, Some(bytes)) => {
                let _ = decode_values(bytes, dimensions)?;
            }
            _ => return Err(corrupt()),
        }
        if let Some(metadata) = &item.metadata {
            let value: Value = serde_json::from_slice(metadata).map_err(|_| corrupt())?;
            if canonical_metadata(&value).map_err(|_| corrupt())? != *metadata {
                return Err(corrupt());
            }
        }
    }
    Ok(())
}

pub(super) fn apply_write(
    tx: &Transaction<'_>,
    item: &StoredItem,
    sequence: u64,
    kind: VectorMutationKind,
) -> Result<(), PlatformError> {
    let values = item.values.as_ref().ok_or_else(corrupt)?;
    let sql = match kind {
        VectorMutationKind::Insert => {
            "INSERT INTO vectors(vector_id, namespace, values_f32le, metadata_json, norm, updated_sequence)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT(vector_id) DO NOTHING"
        }
        VectorMutationKind::Upsert => {
            "INSERT INTO vectors(vector_id, namespace, values_f32le, metadata_json, norm, updated_sequence)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT(vector_id) DO UPDATE SET namespace = excluded.namespace,
               values_f32le = excluded.values_f32le, metadata_json = excluded.metadata_json,
               norm = excluded.norm, updated_sequence = excluded.updated_sequence"
        }
        VectorMutationKind::Delete => return Err(corrupt()),
    };
    let inserted = tx
        .execute(
            sql,
            params![
                item.id,
                item.namespace,
                values,
                item.metadata,
                i64::try_from(sequence).map_err(|_| corrupt())?
            ],
        )
        .map_err(|_| unavailable())?;
    if kind == VectorMutationKind::Insert && inserted == 0 {
        return Ok(());
    }
    let rowid: i64 = tx
        .query_row(
            "SELECT vector_rowid FROM vectors WHERE vector_id = ?1",
            [&item.id],
            |row| row.get(0),
        )
        .map_err(|_| corrupt())?;
    tx.execute(
        "DELETE FROM metadata_terms WHERE vector_rowid = ?1",
        [rowid],
    )
    .map_err(|_| corrupt())?;
    if let Some(metadata) = &item.metadata {
        let value: Value = serde_json::from_slice(metadata).map_err(|_| corrupt())?;
        let mut statement = tx
            .prepare(
                "SELECT property_name, property_type FROM metadata_indexes ORDER BY property_name",
            )
            .map_err(|_| corrupt())?;
        let indexes = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| corrupt())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| corrupt())?;
        drop(statement);
        for (property, property_type) in indexes {
            insert_terms(tx, rowid, &property, &property_type, &value)?;
        }
    }
    Ok(())
}

pub(super) fn insert_terms(
    tx: &Transaction<'_>,
    rowid: i64,
    property: &str,
    property_type: &str,
    metadata: &Value,
) -> Result<(), PlatformError> {
    let Some(value) = resolve_property(metadata, property) else {
        return Ok(());
    };
    let values = value
        .as_array()
        .map_or_else(|| vec![value], |values| values.iter().collect());
    for (ordinal, value) in values.into_iter().enumerate() {
        let (string, number, boolean) = match (property_type, value) {
            ("string", Value::String(value)) => (Some(indexed_prefix(value)), None, None),
            ("number", Value::Number(value)) => {
                (None, value.as_f64().filter(|value| value.is_finite()), None)
            }
            ("boolean", Value::Bool(value)) => (None, None, Some(i64::from(*value))),
            _ => continue,
        };
        if string.is_none() && number.is_none() && boolean.is_none() {
            continue;
        }
        tx.execute(
            "INSERT INTO metadata_terms
             (property_name, vector_rowid, ordinal, string_value, number_value, boolean_value)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                property,
                rowid,
                i64::try_from(ordinal).map_err(|_| invalid())?,
                string,
                number,
                boolean
            ],
        )
        .map_err(|_| corrupt())?;
    }
    Ok(())
}

pub(super) fn read_vector(
    connection: &Connection,
    id: &str,
    dimensions: usize,
) -> Result<Option<VectorRecord>, PlatformError> {
    let row: Option<StoredVectorRow> = connection.query_row(
        "SELECT vector_id, namespace, values_f32le, metadata_json FROM vectors WHERE vector_id = ?1",
        [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    ).optional().map_err(|_| corrupt())?;
    row.map(|(id, namespace, values, metadata)| {
        decode_record(id, namespace, &values, metadata, dimensions)
    })
    .transpose()
}

pub(super) fn decode_record(
    id: String,
    namespace: Option<String>,
    values: &[u8],
    metadata: Option<Vec<u8>>,
    dimensions: usize,
) -> Result<VectorRecord, PlatformError> {
    Ok(VectorRecord {
        id,
        namespace,
        values: decode_values(values, dimensions)?,
        metadata: metadata
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| corrupt()))
            .transpose()?,
    })
}

pub(super) fn encode_values(values: &[f32], dimensions: usize) -> Result<Vec<u8>, PlatformError> {
    if values.len() != dimensions || values.iter().any(|value| !value.is_finite()) {
        return Err(invalid());
    }
    Ok(values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect())
}

pub(super) fn decode_values(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>, PlatformError> {
    if bytes.len() != dimensions.checked_mul(4).ok_or_else(corrupt)? {
        return Err(corrupt());
    }
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| {
            let value = f32::from_le_bytes(*chunk);
            if value.is_finite() {
                Ok(value)
            } else {
                Err(corrupt())
            }
        })
        .collect()
}

pub(super) fn canonical_metadata(value: &Value) -> Result<Vec<u8>, PlatformError> {
    if !value.is_object() {
        return Err(invalid());
    }
    validate_metadata_value(value, 0)?;
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(limit());
    }
    Ok(bytes)
}

pub(super) fn logical_record_bytes(item: &StoredItem) -> Result<u64, PlatformError> {
    let lengths = [
        item.id.len(),
        item.namespace.as_ref().map_or(0, String::len),
        item.values.as_ref().map_or(0, Vec::len),
        item.metadata.as_ref().map_or(0, Vec::len),
    ];
    lengths.into_iter().try_fold(0_u64, |sum, length| {
        sum.checked_add(u64::try_from(length).map_err(|_| limit())?)
            .ok_or_else(limit)
    })
}

pub(super) fn validate_metadata_value(value: &Value, depth: usize) -> Result<(), PlatformError> {
    match value {
        Value::Object(values) if depth <= 1 => {
            for (key, value) in values {
                if key.is_empty() || key.contains('.') {
                    return Err(invalid());
                }
                validate_metadata_value(value, depth + 1)?;
            }
            Ok(())
        }
        Value::String(_) | Value::Bool(_) => Ok(()),
        Value::Number(value) if value.as_f64().is_some_and(f64::is_finite) => Ok(()),
        Value::Array(values) if !values.is_empty() && values.iter().all(Value::is_string) => Ok(()),
        _ => Err(invalid()),
    }
}

pub(super) fn resolve_property<'a>(metadata: &'a Value, property: &str) -> Option<&'a Value> {
    property.split('.').try_fold(metadata, |value, component| {
        value.as_object()?.get(component)
    })
}

pub(super) fn indexed_prefix(value: &str) -> String {
    let mut end = value.len().min(64);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

pub(super) fn candidate_scan_sql(
    namespace: Option<&str>,
    metadata_filter: Option<&FilterExpr>,
) -> Result<(String, Vec<SqlValue>), PlatformError> {
    let mut sql = String::from(
        "SELECT v.vector_id, v.namespace, v.values_f32le, v.metadata_json FROM vectors AS v WHERE 1",
    );
    let mut parameters = Vec::new();
    if let Some(namespace) = namespace {
        sql.push_str(" AND v.namespace = ?");
        parameters.push(SqlValue::Text(namespace.to_string()));
    }
    if let Some(filter) = metadata_filter {
        for predicate in filter.predicates() {
            sql.push_str(" AND (");
            push_sql_predicate(&mut sql, &mut parameters, predicate)?;
            sql.push(')');
        }
    }
    sql.push_str(" ORDER BY v.vector_rowid");
    Ok((sql, parameters))
}

pub(super) fn push_sql_predicate(
    sql: &mut String,
    parameters: &mut Vec<SqlValue>,
    predicate: &FilterPredicate,
) -> Result<(), PlatformError> {
    match predicate.operator() {
        FilterOperator::Equal | FilterOperator::In => {
            let has_null = predicate
                .operands()
                .iter()
                .any(|operand| matches!(operand, MetadataScalar::Null));
            let comparable = predicate
                .operands()
                .iter()
                .filter(|operand| !matches!(operand, MetadataScalar::Null))
                .collect::<Vec<_>>();
            if has_null {
                push_property_missing(sql, parameters, predicate.property_path());
                if !comparable.is_empty() {
                    sql.push_str(" OR ");
                }
            }
            if !comparable.is_empty() {
                push_matching_terms(sql, parameters, predicate.property_path(), &comparable, "=")?;
            }
        }
        FilterOperator::NotEqual | FilterOperator::NotIn => {
            let comparable = predicate
                .operands()
                .iter()
                .filter(|operand| !matches!(operand, MetadataScalar::Null))
                .collect::<Vec<_>>();
            let safe_to_exclude = comparable.iter().all(
                |operand| !matches!(operand, MetadataScalar::String(value) if value.len() >= 64),
            );
            if !comparable.is_empty() && safe_to_exclude {
                sql.push_str("NOT ");
                push_matching_terms(sql, parameters, predicate.property_path(), &comparable, "=")?;
            } else {
                sql.push('1');
            }
        }
        operator => {
            let operand = predicate.operands().first().ok_or_else(corrupt)?;
            let comparison = match operator {
                FilterOperator::LessThan => {
                    if matches!(operand, MetadataScalar::String(value) if value.len() >= 64) {
                        "<="
                    } else {
                        "<"
                    }
                }
                FilterOperator::LessThanOrEqual => "<=",
                FilterOperator::GreaterThan => {
                    if matches!(operand, MetadataScalar::String(value) if value.len() >= 64) {
                        ">="
                    } else {
                        ">"
                    }
                }
                FilterOperator::GreaterThanOrEqual => ">=",
                FilterOperator::Equal
                | FilterOperator::NotEqual
                | FilterOperator::In
                | FilterOperator::NotIn => return Err(corrupt()),
            };
            push_matching_terms(
                sql,
                parameters,
                predicate.property_path(),
                &[operand],
                comparison,
            )?;
        }
    }
    Ok(())
}

pub(super) fn push_property_present(
    sql: &mut String,
    parameters: &mut Vec<SqlValue>,
    property: &str,
) {
    sql.push_str(
        "EXISTS (SELECT 1 FROM metadata_terms AS term WHERE term.vector_rowid = v.vector_rowid AND term.property_name = ?)",
    );
    parameters.push(SqlValue::Text(property.to_string()));
}

pub(super) fn push_property_missing(
    sql: &mut String,
    parameters: &mut Vec<SqlValue>,
    property: &str,
) {
    sql.push_str("NOT ");
    push_property_present(sql, parameters, property);
}

pub(super) fn push_matching_terms(
    sql: &mut String,
    parameters: &mut Vec<SqlValue>,
    property: &str,
    operands: &[&MetadataScalar],
    comparison: &str,
) -> Result<(), PlatformError> {
    sql.push_str(
        "EXISTS (SELECT 1 FROM metadata_terms AS term WHERE term.vector_rowid = v.vector_rowid AND term.property_name = ? AND (",
    );
    parameters.push(SqlValue::Text(property.to_string()));
    for (index, operand) in operands.iter().enumerate() {
        if index != 0 {
            sql.push_str(" OR ");
        }
        let (column, value) = sql_operand(operand)?;
        sql.push_str(column);
        sql.push(' ');
        sql.push_str(comparison);
        sql.push_str(" ?");
        parameters.push(value);
    }
    sql.push_str("))");
    Ok(())
}

pub(super) fn sql_operand(
    operand: &MetadataScalar,
) -> Result<(&'static str, SqlValue), PlatformError> {
    match operand {
        MetadataScalar::String(value) => {
            Ok(("term.string_value", SqlValue::Text(indexed_prefix(value))))
        }
        MetadataScalar::Number(value) if value.is_finite() => {
            Ok(("term.number_value", SqlValue::Real(*value)))
        }
        MetadataScalar::Boolean(value) => {
            Ok(("term.boolean_value", SqlValue::Integer(i64::from(*value))))
        }
        MetadataScalar::Null | MetadataScalar::Number(_) => Err(corrupt()),
    }
}

pub(super) fn valid_identity(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max
}

pub(super) fn valid_property_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .split('.')
            .all(|part| !part.is_empty() && !part.starts_with('$'))
}

pub(super) fn mark_failed(
    tx: &Transaction<'_>,
    id: &str,
    code: &str,
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "UPDATE vector_mutations SET state = 'failed', claim_token = NULL,
         claim_until_ms = NULL, error_code = ?1, completed_at_ms = ?2
         WHERE mutation_id = ?3 AND state IN ('queued', 'claimed')",
        params![code, now_ms, id],
    )
    .map_err(|_| corrupt())?;
    Ok(())
}

pub(super) fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "Vectorize request is invalid",
    )
}

pub(super) fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Vectorize metadata index was not found",
    )
}

pub(super) fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingLimitExceeded,
        "Vectorize request exceeds a fixed limit",
    )
}

pub(super) fn conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNameConflict,
        "Vectorize identity already exists",
    )
}

pub(super) fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Vectorize SQLite authority is corrupt",
    )
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "Vectorize SQLite authority is unavailable",
    )
}

pub(super) fn frontier_blocked() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "Vectorize mutation frontier is blocked",
    )
}
