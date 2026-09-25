//! Vectorize database migration.

use super::corrupt;
use open_compute_core::PlatformError;
use rusqlite::Connection;

pub(super) fn migrate(connection: &mut Connection) -> Result<(), PlatformError> {
    crate::schema_migrations::migrate(
        connection,
        crate::schema_migrations::DatabaseKind::Vectorize,
    )
    .map_err(|_| corrupt())
}
