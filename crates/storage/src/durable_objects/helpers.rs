use super::*;

pub(super) type NamespaceProduct = (WorkerId, String, String, String, u32, i64);

pub(super) fn collect_namespace_list_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> Result<DurableObjectNamespaceRecord, rusqlite::Error>,
    >,
) -> Result<Vec<DurableObjectNamespaceRecord>, PlatformError> {
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|_| db_error())?);
    }
    Ok(records)
}

pub(super) fn map_namespace_list_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DurableObjectNamespaceRecord> {
    let worker: String = row.get(12)?;
    let schema: i64 = row.get(16)?;
    Ok(DurableObjectNamespaceRecord {
        resource: crate::resources::map_resource_offset(row, 0)?,
        owner_worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        class_name: row.get(13)?,
        do_storage_id: row.get(14)?,
        namespace_storage_key: row.get(15)?,
        schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(17)?,
    })
}

pub(super) fn namespace_record(
    resource: ResourceRecord,
    product: NamespaceProduct,
) -> DurableObjectNamespaceRecord {
    DurableObjectNamespaceRecord {
        resource,
        owner_worker_id: product.0,
        class_name: product.1,
        do_storage_id: product.2,
        namespace_storage_key: product.3,
        schema_version: product.4,
        created_at_ms: product.5,
    }
}

pub(super) fn read_namespace_product(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<NamespaceProduct, PlatformError> {
    conn.query_row(
        "SELECT owner_worker_id, class_name, do_storage_id, namespace_storage_key,
                    schema_version, created_at_ms FROM do_namespaces WHERE resource_id = ?1",
        [resource_id.to_string()],
        |row| {
            let worker: String = row.get(0)?;
            let schema: i64 = row.get(4)?;
            Ok((
                WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
                row.get(5)?,
            ))
        },
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(namespace_not_found)
}

pub(super) fn register_object_tx(
    tx: &rusqlite::Transaction<'_>,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
    now_ms: i64,
    allow_create: bool,
) -> Result<DurableObjectRecord, PlatformError> {
    if let Some(current) = read_live_object(tx, namespace_id, object_id)? {
        return match current.state {
            DurableObjectState::Deleting => Err(PlatformError::new(
                ErrorCode::DoObjectDeleting,
                "Durable Object deletion is in progress",
            )),
            DurableObjectState::Creating => Ok(current),
            DurableObjectState::Ready => Ok(current),
            DurableObjectState::Tombstoned => Err(invariant()),
        };
    }
    if !allow_create {
        return Err(PlatformError::new(
            ErrorCode::DoStorageLimit,
            "Durable Object storage stop-writes watermark is active",
        ));
    }
    let prior: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM do_objects
             WHERE namespace_resource_id = ?1 AND object_id = ?2",
            params![namespace_id.to_string(), object_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    let generation = prior.checked_add(1).ok_or_else(invariant)?;
    tx.execute(
        "INSERT INTO do_objects
         (namespace_resource_id, object_id, generation, state,
          created_at_ms, updated_at_ms, deleted_at_ms)
         VALUES (?1, ?2, ?3, 'creating', ?4, ?4, NULL)",
        params![
            namespace_id.to_string(),
            object_id.to_string(),
            generation,
            now_ms
        ],
    )
    .map_err(|_| db_error())?;
    read_object(
        tx,
        namespace_id,
        object_id,
        u64::try_from(generation).map_err(|_| invariant())?,
    )
}

pub(super) fn read_live_object(
    conn: &rusqlite::Connection,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
) -> Result<Option<DurableObjectRecord>, PlatformError> {
    conn.query_row(
        "SELECT namespace_resource_id, object_id, generation, state,
                    created_at_ms, updated_at_ms, deleted_at_ms
             FROM do_objects WHERE namespace_resource_id = ?1 AND object_id = ?2
               AND state != 'tombstoned'",
        params![namespace_id.to_string(), object_id.to_string()],
        map_object,
    )
    .optional()
    .map_err(|_| db_error())
}

pub(super) fn read_object(
    conn: &rusqlite::Connection,
    namespace_id: ResourceId,
    object_id: DurableObjectId,
    generation: u64,
) -> Result<DurableObjectRecord, PlatformError> {
    conn.query_row(
        "SELECT namespace_resource_id, object_id, generation, state,
                    created_at_ms, updated_at_ms, deleted_at_ms
             FROM do_objects WHERE namespace_resource_id = ?1 AND object_id = ?2
               AND generation = ?3",
        params![
            namespace_id.to_string(),
            object_id.to_string(),
            i64::try_from(generation).map_err(|_| invariant())?,
        ],
        map_object,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(invariant)
}

pub(super) fn map_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<DurableObjectRecord> {
    let namespace: String = row.get(0)?;
    let object: String = row.get(1)?;
    let generation: i64 = row.get(2)?;
    let state: String = row.get(3)?;
    Ok(DurableObjectRecord {
        namespace_resource_id: ResourceId::from_str(&namespace)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_id: DurableObjectId::from_str(&object).map_err(|_| rusqlite::Error::InvalidQuery)?,
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: DurableObjectState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        deleted_at_ms: row.get(6)?,
    })
}

pub(super) fn namespace_storage_key(do_storage_id: &str, resource_id: ResourceId) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/do-storage/v1\0");
    digest.update(do_storage_id.as_bytes());
    digest.update(b"\0");
    digest.update(resource_id.as_uuid().as_bytes());
    hex::encode(digest.finalize())
}

pub(super) fn validate_class_name(value: &str) -> Result<(), PlatformError> {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_class());
    };
    if value.len() > 128
        || !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$'))
        || bytes.any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')))
    {
        return Err(invalid_class());
    }
    Ok(())
}

pub(super) fn array32(value: &[u8]) -> Result<[u8; 32], PlatformError> {
    value.try_into().map_err(|_| invariant())
}

pub(super) fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|_| db_error())?);
    }
    Ok(out)
}

pub(super) fn invalid_class() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoClassNotFound,
        "Durable Object class name is invalid",
    )
}

pub(super) fn namespace_not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoNamespaceNotFound,
        "Durable Object namespace authority was not found",
    )
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Durable Object authority invariant failed",
    )
}

pub(super) fn invalid_list_cursor() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "object list cursor is invalid")
}

pub(super) fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Durable Object authority database operation failed",
    )
}
