use super::*;

impl VersionController<'_> {
    pub(super) async fn publish_reserved_workflows(
        &self,
        request: &CreateVersionRequest,
        version: &VersionRecord,
        repo: &WorkerRepository<'_>,
    ) -> Result<(), PlatformError> {
        let workflows = WorkflowRepository::new(self.storage.db());
        let mut staged = Vec::with_capacity(self.workflow_reservations.len());
        for reservation in &self.workflow_reservations {
            let class_name = reservation
                .definition
                .reserved_class_name
                .as_deref()
                .ok_or_else(invariant)?;
            let workflow = workflows.stage_reserved_version(
                request.instance_id,
                reservation.definition.id,
                version.id,
                class_name,
                reservation,
                request.now_ms,
            )?;
            if workflow.state == VersionState::Rejected {
                repo.mark_rejected(
                    version.id,
                    VersionState::Validating,
                    ErrorCode::WorkflowVersionNotReady,
                    request.now_ms,
                )?;
                return Err(PlatformError::new(
                    ErrorCode::WorkflowVersionNotReady,
                    "Workflow class validation previously failed",
                ));
            }
            staged.push(workflow);
        }
        for workflow in &staged {
            if workflow.state == VersionState::Ready {
                continue;
            }
            match self
                .validator
                .validate_workflow(workflow.target.clone())
                .await
            {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::WorkflowVersionNotReady
                            | ErrorCode::ArtifactIntegrityError
                            | ErrorCode::WorkflowInvariantViolation
                    ) =>
                {
                    for staged in &staged {
                        if staged.state == VersionState::Validating {
                            workflows.finish_version(
                                request.instance_id,
                                staged.target.workflow_version_id,
                                false,
                                request.now_ms,
                            )?;
                        }
                    }
                    repo.mark_rejected(
                        version.id,
                        VersionState::Validating,
                        ErrorCode::WorkflowVersionNotReady,
                        request.now_ms,
                    )?;
                    return Err(PlatformError::new(
                        ErrorCode::WorkflowVersionNotReady,
                        "real workerd validation rejected a Workflow class",
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        for workflow in staged {
            if workflow.state == VersionState::Validating {
                workflows.finish_version(
                    request.instance_id,
                    workflow.target.workflow_version_id,
                    true,
                    request.now_ms,
                )?;
            }
        }
        Ok(())
    }
}
