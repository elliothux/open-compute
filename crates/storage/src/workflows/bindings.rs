use super::*;

impl WorkflowBindingDescriptor {
    /// Validate and hash the canonical binding identity used by `RuntimeSource` and the private backend.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        let name = self.name.as_bytes();
        if self.kind != open_compute_core::BindingKind::Workflow
            || self.schema_version != 1
            || self.capability_version != 1
            || self.definition_lifecycle_generation < 1
            || self.schedules.len() > 100
            || name.is_empty()
            || name.len() > 64
            || self.name.starts_with("__")
            || self.name.starts_with("OPEN_COMPUTE_")
            || name[0].is_ascii_digit()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return Err(error(ErrorCode::WorkflowBindingStale));
        }
        if self
            .schedules
            .windows(2)
            .any(|values| values[0] >= values[1])
            || self
                .schedules
                .iter()
                .any(|value| open_compute_core::CronSchedule::parse(value).is_err())
        {
            return Err(error(ErrorCode::WorkflowBindingStale));
        }
        Ok(Sha256::digest(serde_json::to_vec(self).map_err(|_| invariant())?).into())
    }
}

impl WorkflowRepository<'_> {
    /// Prepare an immutable binding from a ready same-account definition, without inserting it yet.
    pub fn prepare_binding(
        &self,
        account: AccountId,
        deployment: DeploymentId,
        name: &str,
        definition: WorkflowId,
        mut schedules: Vec<String>,
        now_ms: i64,
    ) -> Result<WorkflowBindingRecord, PlatformError> {
        let definition = self.definition(account, definition)?;
        if definition.state != ResourceState::Ready
            || definition.availability != ResourceAvailability::Healthy
        {
            return Err(error(ErrorCode::WorkflowNotReady));
        }
        schedules.sort();
        schedules.dedup();
        let descriptor = WorkflowBindingDescriptor {
            kind: open_compute_core::BindingKind::Workflow,
            schema_version: 1,
            binding_id: BindingId::generate(),
            name: name.into(),
            definition_id: definition.id,
            definition_lifecycle_generation: definition.lifecycle_generation,
            capability_version: 1,
            schedules,
        };
        Ok(WorkflowBindingRecord {
            descriptor_sha256: descriptor.sha256()?,
            descriptor,
            deployment_id: deployment,
            created_at_ms: now_ms,
        })
    }

    /// Resolve a ready caller deployment and exact immutable descriptor before any public method.
    pub fn authorize_binding(
        &self,
        id: BindingId,
        deployment: DeploymentId,
        expected: &[u8; 32],
    ) -> Result<(AccountId, WorkflowBindingRecord), PlatformError> {
        self.db.with_read(|conn| {
            let binding = conn.query_row(&format!("{BINDING_SELECT} JOIN worker_deployments d ON d.id=b.deployment_id
                JOIN workflow_definitions f ON f.id=b.definition_id JOIN workers w ON w.id=d.worker_id
                WHERE b.id=?1 AND b.deployment_id=?2 AND d.state='ready' AND w.account_id=f.account_id
                AND b.definition_lifecycle_generation=f.lifecycle_generation"),params![id.to_string(),deployment.to_string()],binding_row)
                .optional().map_err(sql_error)?.ok_or_else(||error(ErrorCode::WorkflowBindingStale))?;
            if binding.descriptor_sha256 != *expected || binding.descriptor.sha256()? != *expected { return Err(error(ErrorCode::WorkflowBindingStale)); }
            let account = conn.query_row("SELECT account_id FROM workflow_definitions WHERE id=?1",
                [binding.descriptor.definition_id.to_string()],|row|parse(row,0)).map_err(sql_error)?;
            Ok((account,binding))
        })
    }

    /// Prepare one binding mutation or replay its exact durably recorded response.
    pub fn begin_binding_operation(
        &self,
        binding: BindingId,
        operation: WorkflowOperationId,
        kind: &str,
        fingerprint: &[u8; 32],
        request_json: &[u8],
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        if kind.is_empty()
            || kind.len() > 32
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
            || request_json.len() > 2 * 1024 * 1024
        {
            return Err(error(ErrorCode::WorkflowMethodUnsupported));
        }
        self.db.with_immediate(|tx| {
            let existing = tx.query_row(
                "SELECT binding_id,kind,fingerprint,request_json,state,response_json
                 FROM workflow_binding_operations WHERE operation_id=?1",
                [operation.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?, row.get::<_, String>(4)?, row.get::<_, Option<Vec<u8>>>(5)?)),
            ).optional().map_err(sql_error)?;
            if let Some((stored_binding, stored_kind, stored_fingerprint, stored_request, state, response)) = existing {
                if stored_binding != binding.to_string() || stored_kind != kind
                    || stored_fingerprint.as_slice() != fingerprint || stored_request != request_json
                {
                    return Err(invariant());
                }
                return match (state.as_str(), response) {
                    ("prepared", None) => {
                        let locked: bool = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM workflow_binding_operation_locks
                             WHERE binding_id=?1 AND operation_id=?2)",
                            params![binding.to_string(), operation.to_string()],
                            |row| row.get(0),
                        ).map_err(sql_error)?;
                        if !locked { return Err(invariant()); }
                        Ok(None)
                    }
                    ("applied", Some(response)) => Ok(Some(response)),
                    _ => Err(invariant()),
                };
            }
            tx.execute(
                "INSERT INTO workflow_binding_operations(operation_id,binding_id,kind,fingerprint,request_json,state,
                 response_json,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,'prepared',NULL,?6,?6)",
                params![operation.to_string(), binding.to_string(), kind, fingerprint.as_slice(), request_json, now_ms],
            ).map_err(sql_error)?;
            tx.execute(
                "INSERT INTO workflow_binding_operation_locks(binding_id,operation_id,created_at_ms) VALUES(?1,?2,?3)",
                params![binding.to_string(), operation.to_string(), now_ms],
            ).map_err(sql_error)?;
            Ok(None)
        })
    }

    /// Commit the exact response before acknowledging a binding mutation to workerd.
    pub fn finish_binding_operation(
        &self,
        binding: BindingId,
        operation: WorkflowOperationId,
        response_json: &[u8],
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if response_json.len() > 2 * 1024 * 1024 {
            return Err(error(ErrorCode::WorkflowResultTooLarge));
        }
        self.db.with_immediate(|tx| {
            let state = tx.query_row(
                "SELECT state,response_json FROM workflow_binding_operations
                 WHERE operation_id=?1 AND binding_id=?2",
                params![operation.to_string(), binding.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
            ).optional().map_err(sql_error)?.ok_or_else(invariant)?;
            if state.0 == "applied" {
                return if state.1.as_deref() == Some(response_json) {
                    Ok(())
                } else {
                    Err(invariant())
                };
            }
            if state.0 != "prepared" || state.1.is_some() {
                return Err(invariant());
            }
            let changed = tx.execute(
                "UPDATE workflow_binding_operations SET state='applied',response_json=?3,updated_at_ms=?4
                 WHERE operation_id=?1 AND binding_id=?2 AND state='prepared'",
                params![operation.to_string(), binding.to_string(), response_json, now_ms],
            ).map_err(sql_error)?;
            if changed != 1 { return Err(invariant()); }
            let removed = tx.execute(
                "DELETE FROM workflow_binding_operation_locks WHERE binding_id=?1 AND operation_id=?2",
                params![binding.to_string(), operation.to_string()],
            ).map_err(sql_error)?;
            if removed != 1 { return Err(invariant()); }
            Ok(())
        })
    }
}

pub(crate) fn insert_workflow_bindings(
    tx: &rusqlite::Transaction<'_>,
    deployment: DeploymentId,
    bindings: &[WorkflowBindingRecord],
) -> Result<(), PlatformError> {
    for binding in bindings {
        let descriptor = &binding.descriptor;
        if binding.deployment_id != deployment || descriptor.sha256()? != binding.descriptor_sha256
        {
            return Err(invariant());
        }
        tx.execute("INSERT INTO workflow_bindings(id,deployment_id,name,definition_id,definition_lifecycle_generation,
            capability_version,schedules_json,descriptor_sha256,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![
            descriptor.binding_id.to_string(),binding.deployment_id.to_string(),descriptor.name,descriptor.definition_id.to_string(),
            descriptor.definition_lifecycle_generation,descriptor.capability_version,
            serde_json::to_vec(&descriptor.schedules).map_err(|_| invariant())?,binding.descriptor_sha256.as_slice(),binding.created_at_ms]).map_err(sql_error)?;
    }
    Ok(())
}

pub(crate) fn read_workflow_bindings(
    conn: &rusqlite::Connection,
    deployment: DeploymentId,
) -> Result<Vec<WorkflowBindingRecord>, PlatformError> {
    let mut statement = conn
        .prepare(&format!(
            "{BINDING_SELECT} WHERE b.deployment_id=?1 ORDER BY b.name"
        ))
        .map_err(sql_error)?;
    statement
        .query_map([deployment.to_string()], binding_row)
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)
}

pub(super) const BINDING_SELECT: &str =
    "SELECT b.id,b.deployment_id,b.name,b.definition_id,b.definition_lifecycle_generation,
    b.capability_version,b.schedules_json,b.descriptor_sha256,b.created_at_ms FROM workflow_bindings b";
pub(super) fn binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowBindingRecord> {
    Ok(WorkflowBindingRecord {
        descriptor: WorkflowBindingDescriptor {
            kind: open_compute_core::BindingKind::Workflow,
            schema_version: 1,
            binding_id: parse(row, 0)?,
            name: row.get(2)?,
            definition_id: parse(row, 3)?,
            definition_lifecycle_generation: row.get(4)?,
            capability_version: row.get(5)?,
            schedules: serde_json::from_slice(&row.get::<_, Vec<u8>>(6)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        },
        deployment_id: parse(row, 1)?,
        descriptor_sha256: digest(row, 7)?,
        created_at_ms: row.get(8)?,
    })
}
