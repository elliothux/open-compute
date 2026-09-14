//! Vectorize database migration and exact pre-Refinery head adoption.

use super::{VECTORIZE_SCHEMA_VERSION, corrupt};
use open_compute_core::PlatformError;
use rusqlite::Connection;

pub(super) fn migrate(connection: &mut Connection, resource_id: &str) -> Result<(), PlatformError> {
    crate::schema_migrations::migrate(
        connection,
        crate::schema_migrations::DatabaseKind::Vectorize,
        |legacy| {
            let marker: (String, i64) = legacy
                .query_row(
                    "SELECT resource_id, schema_version FROM index_meta WHERE singleton=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|_| corrupt())?;
            let tables: i64 = legacy
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
                     AND name IN ('index_meta','vectors','metadata_indexes','metadata_terms',
                                  'vector_mutations','vector_mutation_items')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| corrupt())?;
            if marker == (resource_id.to_owned(), i64::from(VECTORIZE_SCHEMA_VERSION))
                && tables == 6
            {
                legacy
                    .execute_batch("ALTER TABLE index_meta DROP COLUMN schema_version;")
                    .map_err(|_| corrupt())
            } else {
                Err(corrupt())
            }
        },
    )
    .map_err(|_| corrupt())
}
