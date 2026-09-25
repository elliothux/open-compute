//! Observability database schema and instance authority checks.

use super::*;

pub(super) fn migrate(connection: &mut Connection) -> Result<(), PlatformError> {
    crate::schema_migrations::migrate(
        connection,
        crate::schema_migrations::DatabaseKind::Observability,
    )
    .map_err(|_| unavailable())?;
    let format: String = connection
        .query_row(
            "SELECT value FROM observability_meta WHERE key='data_format'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| unavailable())?;
    if format != DATA_FORMAT {
        return Err(unavailable());
    }
    Ok(())
}

pub(super) fn quick_check(connection: &Connection) -> Result<(), PlatformError> {
    let status: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|_| unavailable())?;
    if status != "ok"
        || connection
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .map_err(|_| unavailable())?
            .is_some()
    {
        Err(unavailable())
    } else {
        Ok(())
    }
}

pub(super) fn bind_instance(
    connection: &Connection,
    instance_id: InstanceId,
) -> Result<(), PlatformError> {
    let stored: Option<String> = connection
        .query_row(
            "SELECT instance_id FROM observability_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| unavailable())?;
    match stored {
        Some(stored) if InstanceId::from_str(&stored).ok() == Some(instance_id) => Ok(()),
        Some(_) => Err(unavailable()),
        None => {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM observability_invocations",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| unavailable())?;
            if count != 0 {
                return Err(unavailable());
            }
            connection
                .execute(
                    "INSERT INTO observability_identity(singleton,instance_id) VALUES(1,?1)",
                    [instance_id.as_str()],
                )
                .map_err(|_| unavailable())?;
            Ok(())
        }
    }
}
