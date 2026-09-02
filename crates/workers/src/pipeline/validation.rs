//! Version authority validation and canonical request fingerprints.

use super::*;

pub(crate) fn validate_secret_set(
    secrets: &BTreeMap<String, SecretString>,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlatformError> {
    if secrets.len() > MAX_SECRETS {
        return Err(secret_invalid("version contains too many secrets"));
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
        total = total
            .checked_add(size)
            .ok_or_else(|| secret_invalid("version secrets exceed their configured total size"))?;
        if total > MAX_SECRET_TOTAL_BYTES {
            return Err(secret_invalid(
                "version secrets exceed their configured total size",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_binding_set(
    bindings: &BTreeMap<String, VersionBindingInput>,
    vars: &BTreeMap<String, serde_json::Value>,
    secrets: &BTreeMap<String, SecretString>,
) -> Result<(), PlatformError> {
    if bindings.len() > MAX_VARS {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "version contains too many bindings",
        ));
    }
    for name in bindings.keys() {
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

pub(crate) fn validate_service_set(
    services: &BTreeMap<String, VersionServiceInput>,
    vars: &BTreeMap<String, serde_json::Value>,
    secrets: &BTreeMap<String, SecretString>,
    bindings: &BTreeMap<String, VersionBindingInput>,
) -> Result<(), PlatformError> {
    if services.len() > MAX_VARS {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "version contains too many Service bindings",
        ));
    }
    for (name, service) in services {
        validate_env_name(name)?;
        if name.len() > 64
            || vars.contains_key(name)
            || secrets.contains_key(name)
            || bindings.contains_key(name)
        {
            return Err(PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "Service binding name conflicts with version env",
            ));
        }
        ServiceDescriptorV1::new(
            name.clone(),
            service.target_worker_id,
            service.entrypoint.clone(),
        )?;
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
    request: &CreateVersionRequest,
    content: &PreparedContent,
    vars: &BTreeMap<String, serde_json::Value>,
    version_id: Option<VersionId>,
) -> Result<[u8; 32], PlatformError> {
    let mut canonical = Vec::new();
    frame(&mut canonical, request.account_id.to_string().as_bytes())?;
    frame(&mut canonical, request.worker_id.to_string().as_bytes())?;
    frame(
        &mut canonical,
        version_id
            .as_ref()
            .map(VersionId::as_canonical_str)
            .as_deref()
            .unwrap_or_default()
            .as_bytes(),
    )?;
    frame(&mut canonical, content.kind().as_str().as_bytes())?;
    if let Some(bundle) = content.bundle() {
        frame(&mut canonical, &bundle.sha256())?;
    } else {
        frame(&mut canonical, &[])?;
    }
    if let Some(assets) = content.assets() {
        frame(&mut canonical, &assets.manifest.canonical_bytes()?)?;
        frame(&mut canonical, &assets.routing.canonical_bytes()?)?;
    } else {
        frame(&mut canonical, &[])?;
        frame(&mut canonical, &[])?;
    }
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
        &serde_json::to_vec(&request.services).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.runtime_features).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.queue_consumers).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.crons).map_err(|_| invariant())?,
    )?;
    canonical.push(u8::from(request.promote));
    let mut domain = Sha256::new();
    domain.update(b"open-compute/version-request/v1");
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

fn secret_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::SecretInvalid, message)
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "version descriptor invariant failed",
    )
}
