use super::*;

const COLUMNS: &str = "id,instance_id,application_id,namespace_resource_id,object_id,object_generation,lease_generation,opaque_engine_key,start_identity,source_image_sha256,image_sha256,artifact_sha256,artifact_size,name,state,reserved_bytes,created_at_ms,expires_at_ms,last_restored_at_ms,captured_at_ms,put_attempted,image_size,directory_path";

pub(super) fn read_snapshot(
    conn: &Connection,
    id: ResourceId,
) -> Result<ContainerSnapshot, PlatformError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM container_snapshots WHERE id=?1"),
        [id],
        decode,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

pub(super) fn snapshots_conn(conn: &Connection) -> Result<Vec<ContainerSnapshot>, PlatformError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM container_snapshots ORDER BY id"
        ))
        .map_err(|_| invariant())?;
    stmt.query_map([], decode)
        .map_err(|_| invariant())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| invariant())
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContainerSnapshot> {
    fn digest(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<[u8; 32]>> {
        row.get::<_, Option<Vec<u8>>>(column)?
            .map(|bytes| bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()
    }
    let namespace = row.get(3)?;
    let object: DurableObjectId = row
        .get::<_, String>(4)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    if !object.belongs_to(namespace) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let state = match row.get::<_, String>(14)?.as_str() {
        "pending" => ContainerSnapshotState::Pending,
        "ready" => ContainerSnapshotState::Ready,
        "retiring" => ContainerSnapshotState::Retiring,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let image_size: Option<u64> = row.get(21)?;
    let captured_at_ms: Option<i64> = row.get(19)?;
    if (state == ContainerSnapshotState::Ready && image_size.is_none())
        || image_size.is_some() != captured_at_ms.is_some()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(ContainerSnapshot {
        id: row.get(0)?,
        instance_id: row.get(1)?,
        application_id: row.get(2)?,
        source: ContainerSnapshotSource {
            identity: ContainerLeaseIdentity {
                namespace_resource_id: namespace,
                object_id: object,
                object_generation: row.get(5)?,
                lease_generation: row.get(6)?,
            },
            opaque_engine_key: row.get(7)?,
            start_identity: row.get(8)?,
        },
        source_image_sha256: digest(row, 9)?.ok_or(rusqlite::Error::InvalidQuery)?,
        image_sha256: digest(row, 10)?,
        artifact_sha256: digest(row, 11)?,
        artifact_size: row.get(12)?,
        image_size,
        name: row.get(13)?,
        directory: row
            .get::<_, Option<String>>(22)?
            .map(open_compute_core::containers::ContainerDirectoryPath::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state,
        reserved_bytes: row.get(15)?,
        created_at_ms: row.get(16)?,
        expires_at_ms: row.get(17)?,
        last_restored_at_ms: row.get(18)?,
        captured_at_ms,
        put_attempted: row.get(20)?,
    })
}
