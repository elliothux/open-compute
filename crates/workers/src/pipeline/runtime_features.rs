use super::*;

#[allow(
    clippy::type_complexity,
    reason = "the callable signature directly models the runtime protocol"
)]
pub(super) fn prepare_runtime_features(
    input: &VersionRuntimeFeatures,
) -> Result<
    (
        CachePolicyDescriptorV1,
        Vec<VersionCachePolicyRecord>,
        Vec<BuiltinBindingDescriptorV1>,
        Vec<VersionBuiltinBindingRecord>,
    ),
    PlatformError,
> {
    let cache_policy = CachePolicyDescriptorV1 {
        enabled: input.cache.default.enabled,
        cross_version_cache: input.cache.default.cross_version_cache,
        entrypoints: input
            .cache
            .entrypoints
            .iter()
            .map(|(name, policy)| {
                (
                    name.clone(),
                    CacheEntrypointPolicyV1 {
                        enabled: policy.enabled,
                        cross_version_cache: policy.cross_version_cache,
                    },
                )
            })
            .collect(),
    };
    cache_policy.validate()?;
    let mut cache_rows = vec![VersionCachePolicyRecord {
        entrypoint: None,
        enabled: cache_policy.enabled,
        cross_version_cache: cache_policy.cross_version_cache,
    }];
    cache_rows.extend(cache_policy.entrypoints.iter().map(|(name, policy)| {
        VersionCachePolicyRecord {
            entrypoint: Some(name.clone()),
            enabled: policy.enabled,
            cross_version_cache: policy.cross_version_cache,
        }
    }));
    let mut descriptors = Vec::new();
    for name in &input.worker_loaders {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            name.clone(),
            BuiltinBindingDescriptorKindV1::WorkerLoader,
            None,
        )?);
    }
    if let Some(ai) = &input.ai {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            ai.binding.clone(),
            BuiltinBindingDescriptorKindV1::Ai,
            None,
        )?);
    }
    if let Some(images) = &input.images {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            images.binding.clone(),
            BuiltinBindingDescriptorKindV1::Images,
            None,
        )?);
    }
    if let Some(metadata) = &input.version_metadata {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            metadata.binding.clone(),
            BuiltinBindingDescriptorKindV1::VersionMetadata,
            metadata.tag.clone(),
        )?);
    }
    for (name, binding) in &input.module_bindings {
        let kind = match binding.kind {
            ModuleBindingKind::WasmModule => BuiltinBindingDescriptorKindV1::WasmModule,
            ModuleBindingKind::TextBlob => BuiltinBindingDescriptorKindV1::TextBlob,
            ModuleBindingKind::DataBlob => BuiltinBindingDescriptorKindV1::DataBlob,
        };
        descriptors.push(BuiltinBindingDescriptorV1::new(
            name.clone(),
            kind,
            Some(binding.module.clone()),
        )?);
    }
    descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let rows = descriptors
        .iter()
        .map(|descriptor| {
            Ok(VersionBuiltinBindingRecord {
                name: descriptor.name.clone(),
                kind: match descriptor.kind {
                    BuiltinBindingDescriptorKindV1::WorkerLoader => {
                        BuiltinBindingKind::WorkerLoader
                    }
                    BuiltinBindingDescriptorKindV1::Ai => BuiltinBindingKind::Ai,
                    BuiltinBindingDescriptorKindV1::Images => BuiltinBindingKind::Images,
                    BuiltinBindingDescriptorKindV1::VersionMetadata => {
                        BuiltinBindingKind::VersionMetadata
                    }
                    BuiltinBindingDescriptorKindV1::WasmModule => BuiltinBindingKind::WasmModule,
                    BuiltinBindingDescriptorKindV1::TextBlob => BuiltinBindingKind::TextBlob,
                    BuiltinBindingDescriptorKindV1::DataBlob => BuiltinBindingKind::DataBlob,
                },
                tag: descriptor.tag.clone(),
                descriptor_sha256: descriptor.sha256()?,
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    Ok((cache_policy, cache_rows, descriptors, rows))
}

pub(super) fn validate_compatibility(
    input: &VersionRuntimeFeatures,
) -> Result<Vec<String>, PlatformError> {
    // P6 intentionally certifies only the formal pin's latest date. Supporting an older date
    // requires separate stock-workerd evidence and an explicit capability-range update.
    if input.compatibility_date != crate::WORKER_COMPATIBILITY_DATE {
        return Err(PlatformError::new(
            ErrorCode::CompatibilityUnsupported,
            "compatibility date is outside the certified pinned-workerd range",
        ));
    }
    if !crate::supports_worker_compatibility(&input.compatibility_date, &input.compatibility_flags)
    {
        return Err(PlatformError::new(
            ErrorCode::CompatibilityUnsupported,
            "compatibility flags are outside the fixed pinned-runtime contract",
        ));
    }
    Ok(input.compatibility_flags.clone())
}

pub(super) fn validate_asset_content(
    request: &CreateVersionRequest,
    content: &PreparedContent,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlatformError> {
    let Some(assets) = content.assets() else {
        return Ok(());
    };
    assets.manifest.validate()?;
    assets.routing.validate()?;
    if let Some(binding) = assets.routing.binding.as_deref()
        && (vars.contains_key(binding)
            || request.secrets.contains_key(binding)
            || request.bindings.contains_key(binding))
    {
        return Err(PlatformError::new(
            ErrorCode::BindingTypeMismatch,
            "asset binding conflicts with another version env name",
        ));
    }
    if content.kind() == VersionContentKind::AssetsOnly
        && (!vars.is_empty()
            || !request.secrets.is_empty()
            || !request.bindings.is_empty()
            || !request.queue_consumers.is_empty()
            || !request.crons.is_empty()
            || matches!(
                assets.routing.run_worker_first,
                RunWorkerFirst::All(true) | RunWorkerFirst::Rules(_)
            ))
    {
        return Err(PlatformError::new(
            ErrorCode::AssetConfigUnsupported,
            "assets-only versions cannot declare an execution environment",
        ));
    }
    Ok(())
}

pub(super) fn map_asset_store_error(error: &PlatformError) -> PlatformError {
    match error.code() {
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt => PlatformError::new(
            ErrorCode::AssetIntegrityError,
            "static asset failed integrity verification",
        ),
        ErrorCode::LimitInvalid => PlatformError::new(
            ErrorCode::AssetLimitExceeded,
            "static asset exceeds the configured object limit",
        ),
        _ => PlatformError::new(
            ErrorCode::AssetStorageUnavailable,
            "static asset provider is unavailable",
        ),
    }
}

pub(crate) fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/version-referrer/v1\0");
    hasher.update(account_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}
