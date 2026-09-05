//! Read-only embedded identity checks and explicitly mutating full runtime diagnostics.

use super::{DoctorCheck, bound_value, failed, ok, skipped};
use crate::config_load::LoadedConfig;
use open_compute_artifacts::{ObjectBackend, preflight_object_storage, preflight_r2};
use open_compute_core::ids::{PlatformId, StartupId};
use open_compute_core::{ErrorCode, ObjectStorageKind, PlatformError, Redactor, SystemClock};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, OsJitter, PlatformReleaseMeta,
    StaticConfigCompiler, SupervisorState, WorkerdSupervisor, WorkerdSupervisorOptions,
    embedded_runtime_lock, inspect_embedded_runtime, materialize_embedded_runtime,
};
use open_compute_storage::{DataRootInspect, inspect_durable_object_storage};
use std::sync::Arc;
use std::time::Duration;

pub(super) fn inspect(checks: &mut Vec<DoctorCheck>, loaded: &LoadedConfig) -> Option<String> {
    let result = (|| -> Result<String, PlatformError> {
        let (lock, _) = embedded_runtime_lock()?;
        let present = inspect_embedded_runtime(&loaded.config.data.path.join("runtime"))?;
        checks.push(ok(
            "runtime_binary",
            if present {
                "embedded runtime cache matches the pinned payload"
            } else {
                "embedded runtime is available; materialization has not run"
            },
            Some(bound_value(&lock.expected_version_output, 32)),
        ));
        Ok(lock.expected_version_output)
    })();
    match result {
        Ok(version) => Some(version),
        Err(error) => {
            checks.push(failed(
                "runtime_binary",
                error.code(),
                error.message(),
                None,
            ));
            None
        }
    }
}

pub(super) async fn run_full_extras(
    checks: &mut Vec<DoctorCheck>,
    loaded: &LoadedConfig,
    root: &DataRootInspect,
    backend: Option<&ObjectBackend>,
    platform_id: Option<PlatformId>,
) {
    // The inspection owns an exclusive flock, retained through the complete child lifecycle.
    if !root.holds_inspect_lock() {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::DataDirInUse,
            "exclusive data directory ownership is required",
            None,
        ));
        return;
    }
    match (backend, platform_id) {
        (Some(backend), Some(platform_id)) => {
            match preflight_object_storage(backend, platform_id, StartupId::generate()).await {
                Ok(_) => {
                    checks.push(ok(
                        "object_storage_canary",
                        "object storage preflight canary succeeded",
                        None,
                    ));
                    if backend.kind() == ObjectStorageKind::Local {
                        checks.push(ok(
                            "local_fsync",
                            "local object publication and directory fsync succeeded",
                            None,
                        ));
                    }
                }
                Err(err) => {
                    checks.push(failed(
                        "object_storage_canary",
                        err.code(),
                        err.message(),
                        None,
                    ));
                    if backend.kind() == ObjectStorageKind::Local {
                        checks.push(failed("local_fsync", err.code(), err.message(), None));
                    }
                }
            }
            match preflight_r2(backend, platform_id, StartupId::generate()).await {
                Ok(outcome) => {
                    let value = if outcome.multi_delete {
                        "multi_delete".to_owned()
                    } else {
                        "single_delete_fallback".to_owned()
                    };
                    checks.push(ok(
                        "r2_canary",
                        "R2 provider capability preflight succeeded",
                        Some(value.clone()),
                    ));
                    if backend.kind() == ObjectStorageKind::S3 {
                        checks.push(ok(
                            "s3_provider_capability",
                            "S3 provider supports the required object operations",
                            Some(value),
                        ));
                    }
                }
                Err(err) => {
                    checks.push(failed("r2_canary", err.code(), err.message(), None));
                    if backend.kind() == ObjectStorageKind::S3 {
                        checks.push(failed(
                            "s3_provider_capability",
                            err.code(),
                            err.message(),
                            None,
                        ));
                    }
                }
            }
        }
        _ => checks.push(skipped(
            "object_storage_canary",
            "object storage canary requires connectivity and stored identity",
        )),
    }
    if backend.is_none() || platform_id.is_none() {
        checks.push(skipped(
            "r2_canary",
            "R2 canary requires connectivity and stored identity",
        ));
        checks.push(skipped(
            match loaded.config.object_storage.kind() {
                ObjectStorageKind::Local => "local_fsync",
                ObjectStorageKind::S3 => "s3_provider_capability",
            },
            "backend capability check requires connectivity and stored identity",
        ));
    }

    let runtime_dir = root.root.join("runtime");
    let package =
        match tokio::task::spawn_blocking(move || materialize_embedded_runtime(&runtime_dir)).await
        {
            Ok(Ok(package)) => package,
            Ok(Err(error)) => {
                checks.push(failed("runtime_cycle", error.code(), error.message(), None));
                return;
            }
            Err(_) => {
                checks.push(failed(
                    "runtime_cycle",
                    ErrorCode::RuntimeInvalid,
                    "embedded runtime task failed",
                    None,
                ));
                return;
            }
        };
    let lease_path = root.root.join("runtime/child.lease");
    let runtime = match package
        .verify(
            Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
            &Redactor::new(),
            &lease_path,
        )
        .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            checks.push(failed("runtime_cycle", error.code(), error.message(), None));
            return;
        }
    };
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        package.lock_path(),
        package.assets_dir(),
        root.root.join("runtime"),
        PlatformReleaseMeta {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        Redactor::new(),
    )
    .with_durable_objects_config(loaded.config.durable_objects.clone());
    let Ok(runtime_source) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary runtime-source listener could not be bound",
            None,
        ));
        return;
    };
    let Ok(runtime_source_addr) = runtime_source.local_addr() else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary runtime-source listener address is unavailable",
            None,
        ));
        return;
    };
    let Ok(binding_backend) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary binding-backend listener could not be bound",
            None,
        ));
        return;
    };
    let Ok(binding_backend_addr) = binding_backend.local_addr() else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary binding-backend listener address is unavailable",
            None,
        ));
        return;
    };
    let runtime_external =
        match ExternalServiceAddress::loopback("runtime-source", runtime_source_addr) {
            Ok(external) => external,
            Err(err) => {
                checks.push(failed("runtime_cycle", err.code(), err.message(), None));
                return;
            }
        };
    let binding_external =
        match ExternalServiceAddress::loopback("binding-backend", binding_backend_addr) {
            Ok(external) => external,
            Err(err) => {
                checks.push(failed("runtime_cycle", err.code(), err.message(), None));
                return;
            }
        };
    let observability_external =
        match ExternalServiceAddress::loopback("observability-backend", binding_backend_addr) {
            Ok(external) => external,
            Err(err) => {
                checks.push(failed("runtime_cycle", err.code(), err.message(), None));
                return;
            }
        };
    let Some(platform_id) = platform_id else {
        checks.push(skipped(
            "runtime_cycle",
            "temporary runtime requires stored platform identity",
        ));
        return;
    };
    let do_storage = match inspect_durable_object_storage(
        &loaded.config.data.path,
        &platform_id.to_string(),
        runtime.version_output(),
    ) {
        Ok(path) => path,
        Err(error) => {
            checks.push(failed("runtime_cycle", error.code(), error.message(), None));
            return;
        }
    };
    let directory = match DirectoryServicePath::local("do-storage", &do_storage) {
        Ok(directory) => directory,
        Err(error) => {
            checks.push(failed("runtime_cycle", error.code(), error.message(), None));
            return;
        }
    };
    let supervisor = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: loaded.config.runtime.clone(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(lease_path),
        },
        vec![runtime_external, binding_external, observability_external],
        vec![directory],
        Vec::new(),
    );
    supervisor.start();
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(loaded.config.runtime.startup_timeout_ms);
    let mut rx = supervisor.subscribe();
    let mut ready = false;
    loop {
        if rx.borrow().state == SupervisorState::Running {
            ready = true;
            break;
        }
        if tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    supervisor.begin_drain();
    supervisor.shutdown().await;
    drop((runtime_source, binding_backend));
    if ready {
        checks.push(ok(
            "runtime_cycle",
            "temporary workerd compile start probe stop succeeded",
            None,
        ));
    } else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeExitedBeforeReady,
            "temporary workerd did not become ready",
            None,
        ));
    }
}
