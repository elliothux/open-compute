//! Deployment authority validation and canonical request fingerprints.

use super::*;

pub(crate) fn validate_secret_set(
    secrets: &BTreeMap<String, SecretString>,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlatformError> {
    if secrets.len() > MAX_SECRETS {
        return Err(secret_invalid("deployment contains too many secrets"));
    }
    let mut total = 0_usize;
    for (name, value) in secrets {
        validate_env_name(name)?;
        if vars.contains_key(name) {
            return Err(secret_invalid("var and secret env names conflict"));
        }
        let size = value.expose().len();
        if size == 0 || size > MAX_SECRET_BYTES {
            return Err(secret_invalid("secret value exceeds its configured size"));
        }
        total = total.checked_add(size).ok_or_else(|| {
            secret_invalid("deployment secrets exceed their configured total size")
        })?;
        if total > MAX_SECRET_TOTAL_BYTES {
            return Err(secret_invalid(
                "deployment secrets exceed their configured total size",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_binding_set(
    bindings: &BTreeMap<String, DeploymentBindingInput>,
    vars: &BTreeMap<String, serde_json::Value>,
    secrets: &BTreeMap<String, SecretString>,
) -> Result<(), PlatformError> {
    if bindings.len() > MAX_VARS {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "deployment contains too many bindings",
        ));
    }
    for (name, binding) in bindings {
        if binding.capability_version != 1
            && !(binding.kind == BindingKind::Workflow && binding.capability_version == 2)
        {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "binding capability is unsupported for this product",
            ));
        }
        validate_env_name(name)?;
        if name.len() > 64 || vars.contains_key(name) || secrets.contains_key(name) {
            return Err(PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "binding env name is invalid or conflicts with var or secret",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_injection_module_collisions(
    manifest: &WorkerBundleManifest,
) -> Result<(), PlatformError> {
    if manifest
        .modules
        .iter()
        .any(|module| module.name.starts_with(SYSTEM_MODULE_PREFIX))
    {
        return Err(PlatformError::new(
            ErrorCode::BundleInvalid,
            "tenant bundle collides with a reserved system module",
        ));
    }
    Ok(())
}

pub(super) fn request_fingerprint(
    request: &CreateDeploymentRequest,
    bundle: &PreparedBundle,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<[u8; 32], PlatformError> {
    let mut canonical = Vec::new();
    frame(&mut canonical, request.account_id.to_string().as_bytes())?;
    frame(&mut canonical, request.worker_id.to_string().as_bytes())?;
    frame(&mut canonical, &bundle.sha256())?;
    frame(&mut canonical, request.compatibility_date.as_bytes())?;
    let mut flags = request.compatibility_flags.clone();
    flags.sort();
    flags.dedup();
    frame(
        &mut canonical,
        &serde_json::to_vec(&flags).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(vars).map_err(|_| invariant())?,
    )?;
    for (name, value) in &request.secrets {
        frame(&mut canonical, name.as_bytes())?;
        frame(&mut canonical, value.expose().as_bytes())?;
    }
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.bindings).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.queue_consumers).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.crons).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.limits).map_err(|_| invariant())?,
    )?;
    canonical.push(u8::from(request.promote));
    let mut domain = Sha256::new();
    domain.update(b"open-compute/deployment-request/v1");
    domain.update(request.account_id.as_uuid().as_bytes());
    domain.update(&canonical);
    let digest: [u8; 32] = domain.finalize().into();
    // This unkeyed digest is only an input to the master-key-derived HMAC.
    canonical.zeroize();
    Ok(digest)
}

fn frame(out: &mut Vec<u8>, value: &[u8]) -> Result<(), PlatformError> {
    let len = u64::try_from(value.len()).map_err(|_| invariant())?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn validate_idempotency_key(key: &str) -> Result<(), PlatformError> {
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "idempotency key is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn stable_validation_code(error: &PlatformError) -> ErrorCode {
    match error.code() {
        ErrorCode::RuntimeUnavailable | ErrorCode::RuntimeResultUnknown => error.code(),
        ErrorCode::ResourceLimitExceeded => ErrorCode::ResourceLimitExceeded,
        _ => ErrorCode::BundleRuntimeInvalid,
    }
}

pub(crate) fn parse_failure_code(code: &str) -> ErrorCode {
    match code {
        "ACCOUNT_NOT_FOUND" => ErrorCode::AccountNotFound,
        "WORKER_NOT_FOUND" => ErrorCode::WorkerNotFound,
        "WORKER_DELETED" => ErrorCode::WorkerDeleted,
        "DEPLOYMENT_NOT_FOUND" => ErrorCode::DeploymentNotFound,
        "DEPLOYMENT_NOT_READY" => ErrorCode::DeploymentNotReady,
        "DEPLOYMENT_INVARIANT_VIOLATION" => ErrorCode::DeploymentInvariantViolation,
        "BUNDLE_INVALID" => ErrorCode::BundleInvalid,
        "BUNDLE_TOO_LARGE" => ErrorCode::BundleTooLarge,
        "BUNDLE_RUNTIME_INVALID" => ErrorCode::BundleRuntimeInvalid,
        "COMPATIBILITY_UNSUPPORTED" => ErrorCode::CompatibilityUnsupported,
        "ARTIFACT_UNAVAILABLE" => ErrorCode::ArtifactUnavailable,
        "ARTIFACT_INTEGRITY_ERROR" => ErrorCode::ArtifactIntegrityError,
        "SECRET_INVALID" => ErrorCode::SecretInvalid,
        "RESOURCE_LIMIT_EXCEEDED" => ErrorCode::ResourceLimitExceeded,
        "RESOURCE_NOT_FOUND" => ErrorCode::ResourceNotFound,
        "RESOURCE_NAME_CONFLICT" => ErrorCode::ResourceNameConflict,
        "RESOURCE_NOT_READY" => ErrorCode::ResourceNotReady,
        "RESOURCE_REFERENCED" => ErrorCode::ResourceReferenced,
        "RESOURCE_UNAVAILABLE" => ErrorCode::ResourceUnavailable,
        "RESOURCE_INVARIANT_VIOLATION" => ErrorCode::ResourceInvariantViolation,
        "BINDING_NOT_FOUND" => ErrorCode::BindingNotFound,
        "BINDING_TYPE_MISMATCH" => ErrorCode::BindingTypeMismatch,
        "BINDING_PERMISSION_DENIED" => ErrorCode::BindingPermissionDenied,
        "BINDING_CAPABILITY_UNSUPPORTED" => ErrorCode::BindingCapabilityUnsupported,
        "BINDING_PROTOCOL_ERROR" => ErrorCode::BindingProtocolError,
        "BINDING_LIMIT_EXCEEDED" => ErrorCode::BindingLimitExceeded,
        "BINDING_RESULT_UNKNOWN" => ErrorCode::BindingResultUnknown,
        "QUEUE_NOT_FOUND" => ErrorCode::QueueNotFound,
        "QUEUE_NOT_READY" => ErrorCode::QueueNotReady,
        "QUEUE_CONFIG_PENDING" => ErrorCode::QueueConfigPending,
        "QUEUE_INVARIANT_VIOLATION" => ErrorCode::QueueInvariantViolation,
        "RUNTIME_UNAVAILABLE" => ErrorCode::RuntimeUnavailable,
        "RUNTIME_RESULT_UNKNOWN" => ErrorCode::RuntimeResultUnknown,
        _ => ErrorCode::Internal,
    }
}

fn secret_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::SecretInvalid, message)
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "deployment descriptor invariant failed",
    )
}
