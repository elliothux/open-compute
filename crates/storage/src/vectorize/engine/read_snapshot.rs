//! Consistent read views over one Vectorize SQLite generation.

use super::persistence::*;
use super::{MAX_BATCH_ITEMS, MAX_ID_BYTES, MAX_NAMESPACE_BYTES, VectorRecord, VectorizeEngine};
use open_compute_core::PlatformError;
use open_compute_search::FilterExpr;
use rusqlite::{Connection, params_from_iter};

/// Read-only view whose operations share one SQLite transaction snapshot.
///
/// SQLite establishes the snapshot on the first read. The view cannot outlive the callback passed
/// to [`VectorizeEngine::with_read_snapshot`], so candidate scanning and result materialization can
/// be kept on the same visible vector generation while concurrent mutations continue in WAL mode.
#[derive(Debug)]
pub struct VectorizeReadSnapshot<'a> {
    connection: &'a Connection,
    dimensions: usize,
}

impl VectorizeReadSnapshot<'_> {
    /// Read applied records for IDs in request order from this snapshot.
    pub fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>, PlatformError> {
        get_by_ids_from_connection(self.connection, self.dimensions, ids)
    }

    /// Stream applied candidates in stable row order from this snapshot.
    pub fn scan_candidates(
        &self,
        namespace: Option<&str>,
        metadata_filter: Option<&FilterExpr>,
        visit: impl FnMut(VectorRecord) -> Result<(), PlatformError>,
    ) -> Result<u64, PlatformError> {
        scan_candidates_from_connection(
            self.connection,
            self.dimensions,
            namespace,
            metadata_filter,
            visit,
        )
    }
}

impl VectorizeEngine {
    /// Run related reads against one SQLite transaction snapshot.
    pub fn with_read_snapshot<T>(
        &self,
        operation: impl FnOnce(&VectorizeReadSnapshot<'_>) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(|_| unavailable())?;
        let result = {
            let snapshot = VectorizeReadSnapshot {
                connection: &transaction,
                dimensions: self.dimensions,
            };
            operation(&snapshot)
        };
        let rollback = transaction.rollback().map_err(|_| unavailable());
        match result {
            Ok(value) => {
                rollback?;
                Ok(value)
            }
            Err(error) => {
                let _ = rollback;
                Err(error)
            }
        }
    }
}

pub(super) fn get_by_ids_from_connection(
    connection: &Connection,
    dimensions: usize,
    ids: &[String],
) -> Result<Vec<VectorRecord>, PlatformError> {
    if ids.len() > MAX_BATCH_ITEMS || ids.iter().any(|id| !valid_identity(id, MAX_ID_BYTES)) {
        return Err(invalid());
    }
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(record) = read_vector(connection, id, dimensions)? {
            records.push(record);
        }
    }
    Ok(records)
}

pub(super) fn scan_candidates_from_connection(
    connection: &Connection,
    dimensions: usize,
    namespace: Option<&str>,
    metadata_filter: Option<&FilterExpr>,
    mut visit: impl FnMut(VectorRecord) -> Result<(), PlatformError>,
) -> Result<u64, PlatformError> {
    if namespace.is_some_and(|value| !valid_identity(value, MAX_NAMESPACE_BYTES)) {
        return Err(invalid());
    }
    let (sql, parameters) = candidate_scan_sql(namespace, metadata_filter)?;
    let mut statement = connection.prepare(&sql).map_err(|_| corrupt())?;
    let mut rows = statement
        .query(params_from_iter(parameters))
        .map_err(|_| corrupt())?;
    let mut visited = 0_u64;
    while let Some(row) = rows.next().map_err(|_| corrupt())? {
        let values = row.get::<_, Vec<u8>>(2).map_err(|_| corrupt())?;
        let record = decode_record(
            row.get(0).map_err(|_| corrupt())?,
            row.get(1).map_err(|_| corrupt())?,
            &values,
            row.get(3).map_err(|_| corrupt())?,
            dimensions,
        )?;
        visit(record)?;
        visited = visited.checked_add(1).ok_or_else(limit)?;
    }
    Ok(visited)
}
