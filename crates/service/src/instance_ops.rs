//! Operator commands for managed instance lifecycle.

use crate::config_discover::discover_and_load_config;
use crate::config_load::load_platform_config_from;
use crate::instance_control::{
    probe_status, read_descriptor, request_login_code, request_shutdown, runtime_dir_for,
};
use crate::instance_registry::{InstanceRecord, InstanceRegistry, ServiceScope};
use crate::service_manager::ServiceManager;
use open_compute_core::{ErrorCode, InstanceId, InstanceSelector, PlatformError};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

/// Bounded wait after managed start/restart before treating the instance as ready.
pub const INSTANCE_READY_TIMEOUT: Duration = Duration::from_secs(60);
const INSTANCE_READY_POLL: Duration = Duration::from_millis(100);

/// List registered instances with live control and service-manager state.
pub fn write_instances(
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let records = registry.list()?;
    let rows = records
        .iter()
        .map(|record| inspect_instance(record, manager, runtime_root))
        .collect::<Result<Vec<_>, PlatformError>>()?;
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "instances",
            "instances": rows.iter().map(|row| serde_json::json!({
                "instance_id": row.record.instance_id,
                "state": row.state,
                "version": row.descriptor.as_ref().map(|value| &value.release_version),
                "config": row.record.canonical_config_path,
                "listener": row.descriptor.as_ref().and_then(|value| value.public_listener.as_ref()),
                "service": row.record.service_identifier,
                "service_scope": row.record.service_scope.as_str(),
            })).collect::<Vec<_>>(),
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else if rows.is_empty() {
        writeln!(out, "No registered instances.").map_err(|_| io_failed())?;
    } else {
        writeln!(out, "ID  STATE  VERSION  CONFIG  LISTENER  SERVICE").map_err(|_| io_failed())?;
        for row in rows {
            let version = row
                .descriptor
                .as_ref()
                .map_or("-", |value| value.release_version.as_str());
            let listener = row
                .descriptor
                .as_ref()
                .and_then(|value| value.public_listener.as_deref())
                .unwrap_or("-");
            writeln!(
                out,
                "{}  {}  {}  {}  {}  {}",
                row.record.instance_id,
                row.state,
                version,
                row.record.canonical_config_path,
                listener,
                row.record.service_identifier,
            )
            .map_err(|_| io_failed())?;
        }
    }
    Ok(())
}

struct InspectedInstance<'a> {
    record: &'a InstanceRecord,
    descriptor: Option<crate::instance_control::GenerationDescriptor>,
    state: &'static str,
}

fn inspect_instance<'a>(
    record: &'a InstanceRecord,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
) -> Result<InspectedInstance<'a>, PlatformError> {
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    let descriptor = probe_status(&runtime);
    let disk_descriptor = read_descriptor(&runtime)?;
    let active = manager.is_active(record);
    let (descriptor, state) = match (descriptor, active) {
        (Ok(Some(descriptor)), Ok(_)) => {
            let state = match descriptor.readiness.as_str() {
                "ready" if descriptor_http_ready(&descriptor) => "ready",
                "ready" => "degraded",
                "degraded" => "degraded",
                "failed" => "failed",
                _ => "starting",
            };
            (Some(descriptor), state)
        }
        (Ok(None), Ok(true)) => (disk_descriptor, "starting"),
        (Ok(None), Ok(false)) if disk_descriptor.is_some() => (disk_descriptor, "stale"),
        (Ok(None), Ok(false)) => (None, "stopped"),
        (Err(_), Ok(false)) => (disk_descriptor, "stale"),
        (Err(_), Ok(true)) | (_, Err(_)) => (disk_descriptor, "failed"),
    };
    Ok(InspectedInstance {
        record,
        descriptor,
        state,
    })
}

/// Resolve a registered instance for online commands.
///
/// `runtime_root` overrides the OS runtime directory (tests only; production passes `None`).
pub fn resolve_online_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    runtime_root: Option<&Path>,
) -> Result<InstanceRecord, PlatformError> {
    match (config, instance) {
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "--instance and --config are mutually exclusive",
        )),
        (None, Some(selector)) => registry.get(selector),
        (Some(path), None) => {
            let loaded = load_platform_config_from(path, startup_cwd)?;
            let id = InstanceId::from_canonical_config_path(&loaded.path)?;
            let selector: InstanceSelector = id.as_str().parse()?;
            registry.get(&selector)
        }
        (None, None) => select_running_or_discover(startup_cwd, registry, runtime_root),
    }
}

fn select_running_or_discover(
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    runtime_root: Option<&Path>,
) -> Result<InstanceRecord, PlatformError> {
    let mut running = running_instances(registry, runtime_root)?;
    match running.len() {
        1 => Ok(running.remove(0)),
        n if n > 1 => Err(PlatformError::new(
            ErrorCode::InstanceAmbiguous,
            "multiple running instances; retry with an exact --instance selector",
        )),
        _ => {
            let loaded = discover_and_load_config(None, startup_cwd)?;
            let id = InstanceId::from_canonical_config_path(&loaded.path)?;
            let selector: InstanceSelector = id.as_str().parse()?;
            registry.get(&selector).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceNotFound,
                    "no running instance and the discovered config is not registered; run `ocd start`",
                )
            })
        }
    }
}

/// Return every registered instance with a responsive live control socket.
pub(crate) fn running_instances(
    registry: &InstanceRegistry,
    runtime_root: Option<&Path>,
) -> Result<Vec<InstanceRecord>, PlatformError> {
    let records = registry.list()?;
    let mut running = Vec::new();
    for record in records {
        let id = record.instance_id()?;
        let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
        if probe_status(&runtime)?.is_some() {
            running.push(record);
        }
    }
    Ok(running)
}

/// Register (if needed), install, enable, and start a managed instance.
pub fn start_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    if instance.is_some() && config.is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "--instance and --config are mutually exclusive",
        ));
    }
    let (loaded, selected_record) = if let Some(selector) = instance {
        let record = registry.get(selector)?;
        (
            load_platform_config_from(record.config_path(), startup_cwd)?,
            Some(record),
        )
    } else {
        (discover_and_load_config(config, startup_cwd)?, None)
    };
    let existing_record = match selected_record {
        Some(record) => Some(record),
        None => registry
            .list()?
            .into_iter()
            .find(|record| record.config_path() == loaded.path),
    };
    let record = if let Some(record) = existing_record {
        record
    } else {
        let scope = if loaded.path.starts_with("/etc/open-compute/") {
            ServiceScope::System
        } else {
            ServiceScope::User
        };
        let service_user = match scope {
            ServiceScope::System => Some(crate::service_manager::system_service_account()?.name),
            ServiceScope::User => None,
        };
        registry.register_with_service_user(
            &loaded.path,
            scope,
            service_user.as_deref(),
            SystemTime::now(),
        )?
    };
    let ocd = std::env::current_exe().map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "failed to resolve the current ocd executable path",
        )
    })?;
    manager.install(&record, &ocd)?;
    manager.enable(&record)?;
    if manager.is_active(&record)? {
        wait_until_instance_ready(
            &record,
            manager.readiness_runtime_root().as_deref(),
            INSTANCE_READY_TIMEOUT,
        )?;
        writeln!(
            out,
            "INSTANCE_OK {} already running {}",
            record.instance_id, record.canonical_config_path
        )
        .map_err(|_| io_failed())?;
        return Ok(());
    }
    manager.start(&record)?;
    wait_until_instance_ready(
        &record,
        manager.readiness_runtime_root().as_deref(),
        INSTANCE_READY_TIMEOUT,
    )?;
    writeln!(
        out,
        "INSTANCE_STARTED {} {}",
        record.instance_id, record.canonical_config_path
    )
    .map_err(|_| io_failed())?;
    Ok(())
}

/// Stop a managed or foreground instance.
pub fn stop_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = resolve_online_instance(config, instance, startup_cwd, registry, runtime_root)?;
    if manager.is_active(&record)? {
        manager.stop(&record)?;
    } else {
        let id = record.instance_id()?;
        let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
        let _ = request_shutdown(&runtime);
    }
    writeln!(out, "INSTANCE_STOPPED {}", record.instance_id).map_err(|_| io_failed())?;
    Ok(())
}

/// Restart a managed instance.
pub fn restart_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = resolve_online_instance(config, instance, startup_cwd, registry, None)?;
    manager.restart(&record)?;
    wait_until_instance_ready(
        &record,
        manager.readiness_runtime_root().as_deref(),
        INSTANCE_READY_TIMEOUT,
    )?;
    writeln!(out, "INSTANCE_RESTARTED {}", record.instance_id).map_err(|_| io_failed())?;
    Ok(())
}

/// Poll control-plane readiness until `ready` or `timeout`.
///
/// Requires both a live control-socket status and an HTTP readiness response.
pub fn wait_until_instance_ready(
    record: &InstanceRecord,
    runtime_root: Option<&Path>,
    timeout: Duration,
) -> Result<(), PlatformError> {
    wait_until_instance_ready_for_release(record, runtime_root, timeout, None)
}

pub(crate) fn wait_until_instance_ready_for_release(
    record: &InstanceRecord,
    runtime_root: Option<&Path>,
    timeout: Duration,
    expected_release: Option<&str>,
) -> Result<(), PlatformError> {
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    let deadline = Instant::now() + timeout;
    loop {
        if instance_reports_ready(&runtime, expected_release).is_ok_and(|ready| ready) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "instance did not become ready before the bounded readiness deadline",
            ));
        }
        std::thread::sleep(INSTANCE_READY_POLL);
    }
}

fn instance_reports_ready(
    runtime: &Path,
    expected_release: Option<&str>,
) -> Result<bool, PlatformError> {
    if let Some(descriptor) = probe_status(runtime)? {
        return Ok(descriptor.readiness == "ready"
            && expected_release.is_none_or(|expected| descriptor.release_version == expected)
            && descriptor_http_ready(&descriptor));
    }
    #[cfg(any(test, feature = "test-support"))]
    if let Some(descriptor) = read_descriptor(runtime)?
        && descriptor.readiness == "ready"
        && expected_release.is_none_or(|expected| descriptor.release_version == expected)
        && descriptor.public_listener.is_none()
        && descriptor.admin_listener.is_none()
    {
        // FakeServiceManager does not own a process or network listener. This
        // test-support-only seam never participates in a production wait.
        return Ok(true);
    }
    Ok(false)
}

fn descriptor_http_ready(descriptor: &crate::instance_control::GenerationDescriptor) -> bool {
    let Some(listener) = descriptor
        .admin_listener
        .as_deref()
        .or(descriptor.public_listener.as_deref())
    else {
        return false;
    };
    let Ok(address) = listener.parse::<SocketAddr>() else {
        return false;
    };
    let connect_address = match address {
        SocketAddr::V4(value) if value.ip().is_unspecified() => {
            SocketAddr::from(([127, 0, 0, 1], value.port()))
        }
        SocketAddr::V6(value) if value.ip().is_unspecified() => {
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], value.port()))
        }
        value => value,
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&connect_address, Duration::from_millis(500))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let host = match address {
        SocketAddr::V4(value) => value.to_string(),
        SocketAddr::V6(value) => format!("[{}]:{}", value.ip(), value.port()),
    };
    if write!(
        stream,
        "GET /health/ready HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .is_err()
    {
        return false;
    }
    let mut response = [0_u8; 64];
    let mut used = 0;
    while used < response.len() && !response[..used].windows(2).any(|bytes| bytes == b"\r\n") {
        let Ok(read) = stream.read(&mut response[used..]) else {
            return false;
        };
        if read == 0 {
            break;
        }
        used += read;
    }
    let status = String::from_utf8_lossy(&response[..used]);
    status.starts_with("HTTP/1.1 200 ") || status.starts_with("HTTP/1.0 200 ")
}

/// Print instance status.
#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub fn status_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let record = resolve_online_instance(config, instance, startup_cwd, registry, runtime_root)?;
    let inspected = inspect_instance(&record, manager, runtime_root)?;
    let descriptor = inspected.descriptor;
    let state = inspected.state;
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "command": "status",
                "instance_id": record.instance_id,
                "state": state,
                "config": record.canonical_config_path,
                "service": record.service_identifier,
                "listener": descriptor.as_ref().and_then(|d| d.public_listener.clone()),
                "readiness": descriptor.as_ref().map(|d| d.readiness.clone()),
            })
        )
        .map_err(|_| io_failed())?;
    } else {
        writeln!(
            out,
            "{}  {}  {}  {}",
            record.instance_id, state, record.canonical_config_path, record.service_identifier
        )
        .map_err(|_| io_failed())?;
    }
    Ok(())
}

/// Print service logs.
pub fn logs_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
    follow: bool,
) -> Result<(), PlatformError> {
    let record = resolve_online_instance(config, instance, startup_cwd, registry, None)?;
    let logs = manager.logs(&record, follow)?;
    write!(out, "{logs}").map_err(|_| io_failed())?;
    Ok(())
}

/// Remove a stopped instance registration and service definition.
pub fn remove_instance(
    instance: &InstanceSelector,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = registry.get(instance)?;
    if manager.is_active(&record)? {
        return Err(PlatformError::new(
            ErrorCode::DataDirInUse,
            "instance is still running; stop it before `instance remove`",
        ));
    }
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    if probe_status(&runtime)?.is_some() {
        return Err(PlatformError::new(
            ErrorCode::DataDirInUse,
            "instance control socket is still live; stop it before `instance remove`",
        ));
    }
    manager.uninstall(&record)?;
    registry.remove(instance)?;
    writeln!(out, "INSTANCE_REMOVED {}", record.instance_id).map_err(|_| io_failed())?;
    Ok(())
}

/// Open the operator Dashboard for a ready instance with a one-time login URL.
#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub fn open_dashboard(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    runtime_root: Option<&Path>,
    no_open: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = resolve_online_instance(config, instance, startup_cwd, registry, runtime_root)?;
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    let descriptor = probe_status(&runtime)?.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "selected instance is not ready; start it before `ocd dashboard`",
        )
    })?;
    let (code, expires_at_ms) = request_login_code(&runtime)?;
    let base = dashboard_base_url(&descriptor)?;
    let url = format!("{base}#login={code}");
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "dashboard",
            "instance_id": record.instance_id,
            "url": url,
            "expires_at_ms": expires_at_ms,
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else {
        writeln!(out, "DASHBOARD_URL {url}").map_err(|_| io_failed())?;
        writeln!(out, "LOGIN_EXPIRES_AT_MS {expires_at_ms}").map_err(|_| io_failed())?;
    }
    if !no_open {
        open_url_in_browser(&url)?;
    }
    Ok(())
}

fn dashboard_base_url(
    descriptor: &crate::instance_control::GenerationDescriptor,
) -> Result<String, PlatformError> {
    let listener = descriptor
        .admin_listener
        .as_deref()
        .or(descriptor.public_listener.as_deref())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "instance descriptor does not advertise a listener",
            )
        })?;
    let host = if listener.starts_with("http://") || listener.starts_with("https://") {
        listener.trim_end_matches('/').to_owned()
    } else {
        format!("http://{listener}")
    };
    Ok(format!("{host}/operator/"))
}

fn open_url_in_browser(url: &str) -> Result<(), PlatformError> {
    let status = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).status()
    } else if cfg!(target_os = "linux") {
        Command::new("xdg-open").arg(url).status()
    } else {
        return Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "opening a browser is not supported on this host; use --no-open",
        ));
    };
    match status {
        Ok(code) if code.success() => Ok(()),
        _ => Err(PlatformError::new(
            ErrorCode::Internal,
            "failed to open the Dashboard URL in a browser; retry with --no-open",
        )),
    }
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
}

#[cfg(test)]
mod tests;
