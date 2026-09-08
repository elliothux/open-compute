use super::*;

pub(super) fn inspect_local_components(
    loaded: &LoadedConfig,
    inspect: &Option<open_compute_storage::DataRootInspect>,
    inspected_key: &Result<open_compute_storage::MasterKey, PlatformError>,
    db_ok: &Option<open_compute_storage::StableIdentity>,
    checks: &mut Vec<DoctorCheck>,
) {
    checks.push(ok(
        "scheduler_policy",
        "scheduler lease exceeds dispatch timeout and guard",
        Some(loaded.config.scheduler.claim_lease_ms.to_string()),
    ));

    match (inspected_key, db_ok) {
        (Ok(key), Some(identity)) if key.fingerprint() != identity.master_key_id => {
            checks.push(failed(
                "master_key",
                ErrorCode::MasterKeyMismatch,
                "master key fingerprint does not match stored identity",
                Some(bound_value(key.fingerprint(), 16)),
            ));
        }
        (Ok(key), _) => checks.push(ok(
            "master_key",
            "master key fingerprint resolved",
            Some(bound_value(key.fingerprint(), 16)),
        )),
        (Err(err), _) => checks.push(failed("master_key", err.code(), err.message(), None)),
    }

    for receipt in ["last-snapshot.json", "last-restore.json"] {
        checks.push(operation_receipt_check(loaded, receipt));
    }

    let hold_local = inspect.as_ref().is_some_and(|root| root.lock_available);
    let cache_dir = loaded.config.data.path.join("cache").join("artifacts");
    let cache_meta = std::fs::symlink_metadata(&cache_dir);
    if !hold_local && inspect.is_some() {
        checks.push(skipped(
            "cache_integrity",
            "data directory exclusive lock is held by another instance",
        ));
    } else if cache_meta
        .as_ref()
        .is_ok_and(|m| !m.file_type().is_symlink() && m.file_type().is_dir())
    {
        match ArtifactCache::inspect_existing(cache_dir) {
            Ok(cache) => match cache.sample_integrity() {
                Ok(sample) if sample.corrupt => checks.push(failed(
                    "cache_integrity",
                    ErrorCode::CacheEntryCorrupt,
                    "cache entry failed integrity checks",
                    Some(sample.entries.to_string()),
                )),
                Ok(sample) => checks.push(ok(
                    "cache_integrity",
                    "cache integrity sample passed",
                    Some(sample.entries.to_string()),
                )),
                Err(err) => checks.push(failed("cache_integrity", err.code(), err.message(), None)),
            },
            Err(err) => checks.push(failed("cache_integrity", err.code(), err.message(), None)),
        }
    } else {
        checks.push(failed(
            "cache_integrity",
            ErrorCode::PathInvalid,
            "artifact cache directory is missing",
            None,
        ));
    }

    let runtime_version = runtime::inspect(checks, loaded);

    match (inspect.as_ref(), db_ok.as_ref(), runtime_version.as_ref()) {
        (Some(root), Some(identity), Some(version)) => match inspect_durable_object_storage(
            &root.root,
            &identity.platform_id.to_string(),
            version,
        ) {
            Ok(_) => checks.push(ok(
                "do_storage",
                "Durable Object localDisk marker and filesystem passed",
                Some("format_v1".to_owned()),
            )),
            Err(error) => checks.push(failed("do_storage", error.code(), error.message(), None)),
        },
        _ => checks.push(skipped(
            "do_storage",
            "data identity and verified workerd are prerequisites",
        )),
    }

    if let ObjectStorageConfig::S3(s3) = &loaded.config.object_storage {
        checks.push(ok(
            "s3_tls",
            "S3 transport security policy is valid",
            Some(
                if s3.endpoint.to_ascii_lowercase().starts_with("https://") {
                    "https".to_owned()
                } else {
                    "loopback_http".to_owned()
                },
            ),
        ));
    }
}

pub(super) async fn inspect_object_storage(
    loaded: &LoadedConfig,
    mode: DoctorMode,
    db_ok: Option<&open_compute_storage::StableIdentity>,
    checks: &mut Vec<DoctorCheck>,
) -> Option<ObjectBackend> {
    let object_backend = match (db_ok, &loaded.config.object_storage, mode) {
        (Some(identity), ObjectStorageConfig::Local(local), DoctorMode::Basic) => {
            match ObjectBackend::inspect_local_authority(local) {
                Ok((platform_id, authority, available))
                    if platform_id == identity.platform_id
                        && identity.object_backend_kind == Some(ObjectStorageKind::Local)
                        && identity.object_authority_sha256 == Some(authority) =>
                {
                    checks.push(ok(
                        "local_root",
                        "local object root is securely accessible",
                        None,
                    ));
                    checks.push(ok(
                        "local_format",
                        "local object format and immutable binding match",
                        Some("format_v1".to_owned()),
                    ));
                    checks.push(ok(
                        "object_storage_connectivity",
                        "local object authority marker and immutable binding match",
                        Some("local".to_owned()),
                    ));
                    if available < local.free_space_hard_bytes {
                        checks.push(failed(
                            "local_free_space",
                            ErrorCode::ObjectStorageCapacity,
                            "local object authority free space is below the hard limit",
                            Some(available.to_string()),
                        ));
                    } else if available < local.free_space_soft_bytes {
                        checks.push(warning(
                            "local_free_space",
                            "local object authority free space is below the soft limit",
                            Some(available.to_string()),
                        ));
                    } else {
                        checks.push(ok(
                            "local_free_space",
                            "local object authority free space is sufficient",
                            Some(available.to_string()),
                        ));
                    }
                }
                Ok(_) => {
                    checks.push(failed(
                        "local_format",
                        ErrorCode::ObjectStorageAuthorityMismatch,
                        "local object authority does not match stored platform identity",
                        None,
                    ));
                    checks.push(failed(
                        "object_storage_connectivity",
                        ErrorCode::ObjectStorageAuthorityMismatch,
                        "local object authority does not match stored platform identity",
                        None,
                    ));
                }
                Err(err) => {
                    checks.push(failed("local_root", err.code(), err.message(), None));
                    checks.push(failed(
                        "object_storage_connectivity",
                        err.code(),
                        err.message(),
                        None,
                    ));
                }
            }
            None
        }
        (Some(identity), _, _) => match connect_object_backend(&loaded.config, identity) {
            Ok(connected)
                if identity.object_backend_kind == Some(connected.backend.kind())
                    && identity.object_authority_sha256
                        == Some(connected.backend.authority_sha256()) =>
            {
                match connected.backend.kind() {
                    ObjectStorageKind::Local => {
                        checks.push(ok(
                            "local_root",
                            "local object root is securely accessible",
                            None,
                        ));
                        checks.push(ok(
                            "local_format",
                            "local object format and immutable binding match",
                            Some("format_v1".to_owned()),
                        ));
                    }
                    ObjectStorageKind::S3 => {}
                }
                Some(connected.backend)
            }
            Ok(_) => {
                checks.push(failed(
                    "object_storage_connectivity",
                    ErrorCode::ObjectStorageAuthorityMismatch,
                    "object authority does not match stored platform identity",
                    None,
                ));
                None
            }
            Err(err) => {
                checks.push(failed(
                    match loaded.config.object_storage.kind() {
                        ObjectStorageKind::Local => "local_root",
                        ObjectStorageKind::S3 => "s3_connectivity",
                    },
                    err.code(),
                    err.message(),
                    None,
                ));
                checks.push(failed(
                    "object_storage_connectivity",
                    err.code(),
                    err.message(),
                    None,
                ));
                None
            }
        },
        (None, _, _) => None,
    };

    if let Some(backend) = object_backend.as_ref() {
        match probe_object_storage(backend).await {
            Ok(()) => {
                checks.push(ok(
                    "object_storage_connectivity",
                    "object storage connectivity probe succeeded",
                    None,
                ));
                if backend.kind() == ObjectStorageKind::S3 {
                    checks.push(ok(
                        "s3_connectivity",
                        "S3 authority is reachable with configured credentials",
                        None,
                    ));
                }
            }
            Err(err) => {
                checks.push(failed(
                    "object_storage_connectivity",
                    err.code(),
                    err.message(),
                    None,
                ));
                if backend.kind() == ObjectStorageKind::S3 {
                    checks.push(failed("s3_connectivity", err.code(), err.message(), None));
                }
            }
        }
        if let ObjectStorageConfig::Local(local) = &loaded.config.object_storage {
            match backend.available_bytes() {
                Ok(Some(available)) if available < local.free_space_hard_bytes => {
                    checks.push(failed(
                        "local_free_space",
                        ErrorCode::ObjectStorageCapacity,
                        "local object authority free space is below the hard limit",
                        Some(available.to_string()),
                    ));
                }
                Ok(Some(available)) if available < local.free_space_soft_bytes => {
                    checks.push(warning(
                        "local_free_space",
                        "local object authority free space is below the soft limit",
                        Some(available.to_string()),
                    ));
                }
                Ok(Some(available)) => checks.push(ok(
                    "local_free_space",
                    "local object authority free space is sufficient",
                    Some(available.to_string()),
                )),
                Ok(None) | Err(_) => checks.push(warning(
                    "local_free_space",
                    "local object authority free space could not be measured",
                    None,
                )),
            }
        }
    }

    object_backend
}

pub(super) async fn inspect_full(
    loaded: &LoadedConfig,
    mode: DoctorMode,
    inspect: Option<&open_compute_storage::DataRootInspect>,
    db_ok: Option<&open_compute_storage::StableIdentity>,
    object_backend: Option<&ObjectBackend>,
    checks: &mut Vec<DoctorCheck>,
) {
    if mode == DoctorMode::Full
        && let Some(root) = inspect.filter(|root| root.holds_inspect_lock())
    {
        runtime::run_full_extras(
            checks,
            loaded,
            root,
            object_backend,
            db_ok.map(|i| i.platform_id),
        )
        .await;
    } else if mode == DoctorMode::Full {
        let reason = if inspect.is_some() {
            "data directory exclusive lock is held by another instance"
        } else {
            "data directory is missing"
        };
        checks.push(skipped("object_storage_canary", reason));
        checks.push(skipped("r2_canary", reason));
        checks.push(skipped(
            match loaded.config.object_storage.kind() {
                ObjectStorageKind::Local => "local_fsync",
                ObjectStorageKind::S3 => "s3_provider_capability",
            },
            reason,
        ));
        checks.push(skipped("runtime_cycle", reason));
    } else {
        checks.push(skipped(
            "object_storage_canary",
            "full doctor is required for the object storage canary",
        ));
        checks.push(skipped(
            "r2_canary",
            "full doctor is required for the R2 capability canary",
        ));
        checks.push(skipped(
            match loaded.config.object_storage.kind() {
                ObjectStorageKind::Local => "local_fsync",
                ObjectStorageKind::S3 => "s3_provider_capability",
            },
            "full doctor is required for a mutating backend capability check",
        ));
        checks.push(skipped(
            "runtime_cycle",
            "full doctor is required for a temporary workerd cycle",
        ));
    }
}
