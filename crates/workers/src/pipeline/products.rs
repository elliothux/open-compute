//! Queue consumer and Cron deployment product validation.

use super::*;

impl DeploymentController<'_> {
    pub(super) fn prepare_queue_consumers(
        &self,
        request: &CreateDeploymentRequest,
    ) -> Result<Vec<NewQueueConsumerDeclaration>, PlatformError> {
        let queues = QueueRepository::new(self.storage.db());
        let mut seen = HashSet::new();
        let mut declarations = Vec::with_capacity(request.queue_consumers.len());
        for input in &request.queue_consumers {
            if !seen.insert(input.queue) {
                return Err(PlatformError::new(
                    ErrorCode::QueueConsumerConflict,
                    "deployment declares the same Queue consumer more than once",
                ));
            }
            validate_entrypoint(input.entrypoint.as_deref())?;
            let config = input.config.validate(self.max_queue_consumer_concurrency)?;
            let source = queues.get(request.account_id, input.queue)?;
            if source.state != QueueState::Ready
                || source.availability != QueueAvailability::Healthy
            {
                return Err(PlatformError::new(
                    ErrorCode::QueueConsumerNotReady,
                    "Queue consumer source is not ready",
                ));
            }
            let dead_letter_queue = input
                .dead_letter_queue
                .map(|id| {
                    if id == source.id {
                        return Err(PlatformError::new(
                            ErrorCode::QueueDlqInvalid,
                            "Queue cannot dead-letter to itself",
                        ));
                    }
                    let target = queues.get(request.account_id, id)?;
                    if target.state != QueueState::Ready
                        || target.availability != QueueAvailability::Healthy
                    {
                        return Err(PlatformError::new(
                            ErrorCode::QueueDlqInvalid,
                            "dead-letter Queue is not ready",
                        ));
                    }
                    Ok((target.id, target.lifecycle_generation))
                })
                .transpose()?;
            let descriptor = serde_json::json!({
                "capabilityVersion": 1,
                "queueId": source.id,
                "queueLifecycleGeneration": source.lifecycle_generation,
                "entrypoint": input.entrypoint,
                "maxBatchSize": config.max_batch_size,
                "maxBatchTimeoutSeconds": config.max_batch_timeout_seconds,
                "maxRetries": config.max_retries,
                "retryDelaySeconds": config.retry_delay_seconds,
                "maxConcurrency": config.max_concurrency,
                "deadLetterQueueId": dead_letter_queue.map(|value| value.0),
                "deadLetterQueueLifecycleGeneration": dead_letter_queue.map(|value| value.1),
            });
            declarations.push(NewQueueConsumerDeclaration {
                id: QueueConsumerId::generate(),
                queue_id: source.id,
                queue_lifecycle_generation: source.lifecycle_generation,
                entrypoint: input.entrypoint.clone(),
                config,
                dead_letter_queue,
                capability_version: 1,
                descriptor_sha256: Sha256::digest(
                    serde_json::to_vec(&descriptor).map_err(|_| invariant())?,
                )
                .into(),
            });
        }
        Ok(declarations)
    }
}

pub(super) fn validate_product_counts(
    request: &CreateDeploymentRequest,
) -> Result<(), PlatformError> {
    if request.queue_consumers.len() > MAX_QUEUE_CONSUMERS_PER_DEPLOYMENT
        || request.crons.len() > MAX_CRONS_PER_DEPLOYMENT
    {
        return Err(PlatformError::new(
            ErrorCode::QuotaExceeded,
            "deployment contains too many Queue consumers or Cron triggers",
        ));
    }
    Ok(())
}

fn validate_entrypoint(entrypoint: Option<&str>) -> Result<(), PlatformError> {
    let Some(value) = entrypoint else {
        return Ok(());
    };
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|value| value.is_ascii_alphabetic() || matches!(value, '_' | '$'));
    if !valid_start
        || value.len() > 128
        || chars.any(|value| !(value.is_ascii_alphanumeric() || matches!(value, '_' | '$')))
    {
        return Err(PlatformError::new(
            ErrorCode::EntrypointNotFound,
            "Queue consumer entrypoint name is invalid",
        ));
    }
    Ok(())
}

pub(super) fn prepare_cron_config(
    request: &CreateDeploymentRequest,
    workflow_bindings: &[open_compute_storage::WorkflowBindingDescriptor],
) -> Result<NewCronConfig, PlatformError> {
    let mut targets: BTreeMap<String, (bool, Vec<String>)> = BTreeMap::new();
    for expression in &request.crons {
        targets.entry(expression.clone()).or_default().0 = true;
    }
    for binding in workflow_bindings {
        for expression in &binding.schedules {
            targets
                .entry(expression.clone())
                .or_default()
                .1
                .push(binding.name.clone());
        }
    }
    if targets.len() > MAX_CRONS_PER_DEPLOYMENT {
        return Err(PlatformError::new(
            ErrorCode::QuotaExceeded,
            "deployment contains too many Cron triggers",
        ));
    }
    let mut declarations = Vec::with_capacity(targets.len());
    for (expression, (scheduled_handler, mut workflow_bindings)) in targets {
        let parsed = CronSchedule::parse(&expression)?;
        workflow_bindings.sort();
        workflow_bindings.dedup();
        declarations.push(NewCronDeclaration {
            id: CronActivationId::generate(),
            expression,
            expression_sha256: Sha256::digest(parsed.normalized().as_bytes()).into(),
            parser_version: CRON_PARSER_VERSION,
            scheduled_handler,
            workflow_bindings,
        });
    }
    let descriptor = serde_json::json!({
        "capabilityVersion": 1,
        "declarations": declarations.iter().map(|declaration| serde_json::json!({
            "expression": declaration.expression,
            "expressionSha256": hex::encode(declaration.expression_sha256),
            "parserVersion": declaration.parser_version,
            "scheduledHandler": declaration.scheduled_handler,
            "workflowBindings": declaration.workflow_bindings,
        })).collect::<Vec<_>>(),
    });
    Ok(NewCronConfig {
        capability_version: 1,
        descriptor_sha256: Sha256::digest(
            serde_json::to_vec(&descriptor).map_err(|_| invariant())?,
        )
        .into(),
        declarations,
    })
}
