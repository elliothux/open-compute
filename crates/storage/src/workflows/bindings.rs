use super::*;

impl WorkflowBindingDescriptor {
    /// Validate and hash the canonical binding identity used by `RuntimeSource` and the private backend.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        let name = self.name.as_bytes();
        if self.kind != open_compute_core::BindingKind::Workflow
            || self.schema_version != 1
            || self.capability_version != 1
            || self.definition_lifecycle_generation < 1
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
        now_ms: i64,
    ) -> Result<WorkflowBindingRecord, PlatformError> {
        let definition = self.definition(account, definition)?;
        if definition.state != ResourceState::Ready
            || definition.availability != ResourceAvailability::Healthy
        {
            return Err(error(ErrorCode::WorkflowNotReady));
        }
        let descriptor = WorkflowBindingDescriptor {
            kind: open_compute_core::BindingKind::Workflow,
            schema_version: 1,
            binding_id: BindingId::generate(),
            name: name.into(),
            definition_id: definition.id,
            definition_lifecycle_generation: definition.lifecycle_generation,
            capability_version: 1,
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
            capability_version,descriptor_sha256,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![
            descriptor.binding_id.to_string(),binding.deployment_id.to_string(),descriptor.name,descriptor.definition_id.to_string(),
            descriptor.definition_lifecycle_generation,descriptor.capability_version,binding.descriptor_sha256.as_slice(),binding.created_at_ms]).map_err(sql_error)?;
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

const BINDING_SELECT: &str =
    "SELECT b.id,b.deployment_id,b.name,b.definition_id,b.definition_lifecycle_generation,
    b.capability_version,b.descriptor_sha256,b.created_at_ms FROM workflow_bindings b";
fn binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowBindingRecord> {
    Ok(WorkflowBindingRecord {
        descriptor: WorkflowBindingDescriptor {
            kind: open_compute_core::BindingKind::Workflow,
            schema_version: 1,
            binding_id: parse(row, 0)?,
            name: row.get(2)?,
            definition_id: parse(row, 3)?,
            definition_lifecycle_generation: row.get(4)?,
            capability_version: row.get(5)?,
        },
        deployment_id: parse(row, 1)?,
        descriptor_sha256: digest(row, 6)?,
        created_at_ms: row.get(7)?,
    })
}
