//! Read-only instance, object, and live-store inspection.

use super::*;

/// Verify an existing database without creating or mutating schema and return
/// the complete secret-free startup authority.
pub fn inspect_ai_search_instance(
    path: &Path,
    resource_id: &str,
    expected_model_sha256: [u8; 32],
    busy_timeout_ms: u64,
) -> Result<AiSearchInstanceAuthority, PlatformError> {
    validate_identity(resource_id)?;
    let connection = open_readonly(path, busy_timeout_ms)?;
    quick_check(&connection)?;
    let row = connection
        .query_row(
            "SELECT schema_version, resource_id, model_contract_sha256,
               previous_model_contract_sha256, model_contract_json, public_config_json,
               dimensions, vector_enabled, keyword_enabled,
               previous_model_contract_json, previous_public_config_json,
               previous_dimensions, previous_vector_enabled, previous_keyword_enabled,
               config_generation, active_index_generation, active_epoch,
               (SELECT COUNT(*) FROM items),
               (SELECT COUNT(*) FROM chunks c JOIN items i ON i.id=c.item_id
                  WHERE c.item_generation=i.active_generation
                    AND c.index_generation=instance_meta.active_index_generation),
               (SELECT COUNT(*) FROM index_jobs
                  WHERE state IN ('queued','claimed','retry_wait','cancelling')),
               transition_model_contract_sha256
             FROM instance_meta WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, Option<Vec<u8>>>(9)?,
                    row.get::<_, Option<Vec<u8>>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<bool>>(12)?,
                    row.get::<_, Option<bool>>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, i64>(18)?,
                    row.get::<_, i64>(19)?,
                    row.get::<_, Option<Vec<u8>>>(20)?,
                ))
            },
        )
        .map_err(sql_error)?;
    let digest = <[u8; 32]>::try_from(row.2).map_err(|_| invariant_error())?;
    let previous_digest = row
        .3
        .map(<[u8; 32]>::try_from)
        .transpose()
        .map_err(|_| invariant_error())?;
    let model_digest: [u8; 32] = Sha256::digest(&row.4).into();
    let transition_digest = row
        .20
        .map(<[u8; 32]>::try_from)
        .transpose()
        .map_err(|_| invariant_error())?;
    let dimensions = u32::try_from(row.6).map_err(|_| invariant_error())?;
    if row.0 != i64::from(AI_SEARCH_SCHEMA_VERSION)
        || row.1 != resource_id
        || (digest != expected_model_sha256
            && previous_digest != Some(expected_model_sha256)
            && transition_digest != Some(expected_model_sha256))
        || model_digest != digest
        || (row.7 && dimensions == 0)
        || (!row.7 && dimensions != 0)
        || (!row.7 && !row.8)
    {
        return Err(invariant_error());
    }
    let contract = AiSearchInstanceStorageContract {
        resource_id,
        model_contract_sha256: digest,
        model_contract_json: &row.4,
        public_config_json: &row.5,
        dimensions,
        vector_enabled: row.7,
        keyword_enabled: row.8,
    };
    if !valid_instance_contract(&contract) {
        return Err(invariant_error());
    }
    let (active_model, active_public) = if let Some(previous_digest) = previous_digest {
        let previous_model = row.9.as_deref().ok_or_else(invariant_error)?;
        let previous_public = row.10.as_deref().ok_or_else(invariant_error)?;
        let previous_dimensions =
            u32::try_from(row.11.ok_or_else(invariant_error)?).map_err(|_| invariant_error())?;
        let previous_contract = AiSearchInstanceStorageContract {
            resource_id,
            model_contract_sha256: previous_digest,
            model_contract_json: previous_model,
            public_config_json: previous_public,
            dimensions: previous_dimensions,
            vector_enabled: row.12.ok_or_else(invariant_error)?,
            keyword_enabled: row.13.ok_or_else(invariant_error)?,
        };
        if Sha256::digest(previous_model).as_slice() != previous_digest
            || !valid_instance_contract(&previous_contract)
        {
            return Err(invariant_error());
        }
        (previous_model.to_vec(), previous_public.to_vec())
    } else {
        (row.4.clone(), row.5.clone())
    };
    Ok(AiSearchInstanceAuthority {
        resource_id: row.1,
        model_contract_sha256: digest,
        dimensions,
        vector_enabled: row.7,
        keyword_enabled: row.8,
        inspection: AiSearchInstanceInspection {
            model_contract_json: active_model,
            public_config_json: active_public,
            indexing_model_contract_json: row.4,
            indexing_public_config_json: row.5,
            config_generation: to_u64(row.14)?,
            active_index_generation: to_u64(row.15)?,
            active_epoch: to_u64(row.16)?,
            item_count: to_u64(row.17)?,
            active_chunk_count: to_u64(row.18)?,
            pending_job_count: to_u64(row.19)?,
            reindex_pending: previous_digest.is_some(),
        },
    })
}

/// Inspect one immutable snapshot copy and enumerate every retained source object exactly.
pub fn inspect_ai_search_object_references(
    path: &Path,
    resource_id: &str,
    busy_timeout_ms: u64,
) -> Result<Vec<AiSearchObjectReference>, PlatformError> {
    validate_identity(resource_id)?;
    let connection = open_readonly(path, busy_timeout_ms)?;
    let valid: bool = connection
        .query_row(
            "SELECT schema_version=?1 AND resource_id=?2 FROM instance_meta WHERE singleton=1",
            params![i64::from(AI_SEARCH_SCHEMA_VERSION), resource_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !valid {
        return Err(invariant_error());
    }
    quick_check(&connection)?;
    object_references(&connection)
}

impl AiSearchStore {
    /// Read bounded secret-free instance counts and frozen contracts in one SQLite snapshot.
    pub fn inspect(&self) -> Result<AiSearchInstanceInspection, PlatformError> {
        let connection = self.lock()?;
        inspect_connection(&connection)
    }

    /// Enumerate exact immutable objects referenced by retained item generations.
    pub fn object_references(&self) -> Result<Vec<AiSearchObjectReference>, PlatformError> {
        let connection = self.lock()?;
        object_references(&connection)
    }
}

fn open_readonly(path: &Path, busy_timeout_ms: u64) -> Result<Connection, PlatformError> {
    crate::fs::validate_owned_file(path, false)?;
    let path = crate::control_db::leaf_nofollow_path(path)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(sql_error)?;
    Ok(connection)
}

fn quick_check(connection: &Connection) -> Result<(), PlatformError> {
    let result: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(sql_error)?;
    if result != "ok" {
        return Err(invariant_error());
    }
    Ok(())
}

fn inspect_connection(
    connection: &Connection,
) -> Result<AiSearchInstanceInspection, PlatformError> {
    let row = connection
        .query_row(
            "SELECT COALESCE(previous_public_config_json, public_config_json),
               COALESCE(previous_model_contract_json, model_contract_json),
               public_config_json, model_contract_json, config_generation,
               active_index_generation, active_epoch, (SELECT COUNT(*) FROM items),
               (SELECT COUNT(*) FROM chunks c JOIN items i ON i.id=c.item_id
                  WHERE c.item_generation=i.active_generation
                    AND c.index_generation=instance_meta.active_index_generation),
               (SELECT COUNT(*) FROM index_jobs
                  WHERE state IN ('queued','claimed','retry_wait','cancelling')),
               previous_model_contract_sha256 IS NOT NULL
             FROM instance_meta WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, bool>(10)?,
                ))
            },
        )
        .map_err(sql_error)?;
    Ok(AiSearchInstanceInspection {
        public_config_json: row.0,
        model_contract_json: row.1,
        indexing_public_config_json: row.2,
        indexing_model_contract_json: row.3,
        config_generation: to_u64(row.4)?,
        active_index_generation: to_u64(row.5)?,
        active_epoch: to_u64(row.6)?,
        item_count: to_u64(row.7)?,
        active_chunk_count: to_u64(row.8)?,
        pending_job_count: to_u64(row.9)?,
        reindex_pending: row.10,
    })
}

fn object_references(
    connection: &Connection,
) -> Result<Vec<AiSearchObjectReference>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT g.object_key, g.object_sha256, g.object_size
             FROM items i JOIN item_generations g ON g.item_id=i.id
              WHERE g.generation IN (i.active_generation, i.desired_generation)
             ORDER BY g.object_key, g.object_sha256, g.object_size",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(sql_error)?;
    let mut references: Vec<AiSearchObjectReference> = Vec::new();
    for row in rows {
        let (object_key, digest, object_size) = row.map_err(sql_error)?;
        if object_key.is_empty()
            || object_key.len() > 1024
            || object_key.starts_with('/')
            || object_key.bytes().any(|byte| byte.is_ascii_control())
            || object_key
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(invariant_error());
        }
        let reference = AiSearchObjectReference {
            object_key,
            object_sha256: digest.try_into().map_err(|_| invariant_error())?,
            object_size: to_u64(object_size)?,
        };
        if let Some(previous) = references.last()
            && previous.object_key == reference.object_key
        {
            if previous != &reference {
                return Err(invariant_error());
            }
            continue;
        }
        references.push(reference);
    }
    Ok(references)
}
