//! Operator commands for managed instance lifecycle.

use crate::config_load::load_platform_config_from;
use crate::instance_control::{probe_status, request_login_code, runtime_dir_for};
use crate::instance_registry::{InstanceRecord, InstanceRegistry, ServiceScope};
use crate::service_manager::ServiceManager;
use open_compute_core::{ErrorCode, InstanceSelector, PlatformError};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Bounded wait after managed start/restart before treating the instance as ready.
pub const INSTANCE_READY_TIMEOUT: Duration = Duration::from_secs(60);
const INSTANCE_READY_POLL: Duration = Duration::from_millis(100);
mod scoped;
pub(crate) use scoped::read_daemon;
pub(crate) use scoped::wait_scoped_daemon_state;
pub use scoped::{
    add_registered_instance, manage_registered_instance, remove_registered_instance, setup_instance,
};
/// Bounded wait for service exit, control-socket removal, and data-lock release.
pub const INSTANCE_STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// One online instance lifecycle action routed to the scoped daemon owner.
#[derive(Clone, Copy, Debug)]
pub enum ScopedInstanceAction {
    /// Start a stopped instance.
    Start,
    /// Stop a running instance.
    Stop,
    /// Stop then start the same instance.
    Restart,
}

/// List instances from the scoped daemon, or the manifest when it is offline.
pub fn write_instances(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let root = registry.root_for(scope);
    let rows = match read_daemon(root)? {
        Some(rows) => rows,
        None => registry
            .list_scope(scope)?
            .into_iter()
            .map(|record| crate::run::daemon_control::InstanceView {
                instance_id: record.instance_id,
                name: record.name,
                state: "stopped".to_owned(),
                error: None,
            })
            .collect(),
    };
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "instances",
            "instances": rows.iter().map(|row| serde_json::json!({
                "instance_id": row.instance_id,
                "name": row.name,
                "state": row.state,
                "error": row.error,
            })).collect::<Vec<_>>(),
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else if rows.is_empty() {
        writeln!(out, "No registered instances.").map_err(|_| io_failed())?;
    } else {
        writeln!(out, "ID  NAME  STATE").map_err(|_| io_failed())?;
        for row in rows {
            writeln!(
                out,
                "{}  {}  {}",
                row.instance_id,
                row.name.as_deref().unwrap_or("-"),
                row.state,
            )
            .map_err(|_| io_failed())?;
        }
    }
    Ok(())
}

/// Print the scoped daemon's liveness without initializing instance data.
pub fn write_daemon_status(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let state = if read_daemon(registry.root_for(scope))?.is_some() {
        "running"
    } else {
        "stopped"
    };
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "command": "status",
                "scope": scope.as_str(),
                "state": state,
            })
        )
        .map_err(|_| io_failed())
    } else {
        writeln!(out, "OCD_STATUS {} {}", scope.as_str(), state).map_err(|_| io_failed())
    }
}

/// Resolve a registered instance for online commands.
///
/// `runtime_root` overrides the OS runtime directory (tests only; production passes `None`).
pub fn resolve_online_instance(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    scope: ServiceScope,
    runtime_root: Option<&Path>,
) -> Result<InstanceRecord, PlatformError> {
    match (config, instance) {
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "--instance and --config are mutually exclusive",
        )),
        (None, Some(selector)) => registry.get_scope(scope, selector),
        (Some(path), None) => {
            let loaded = load_platform_config_from(path, startup_cwd)?;
            registry.get_by_config_scope(scope, &loaded.path)
        }
        (None, None) => select_running(registry, scope, runtime_root),
    }
}

fn select_running(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    runtime_root: Option<&Path>,
) -> Result<InstanceRecord, PlatformError> {
    let mut running = running_instances(registry, scope, runtime_root)?;
    match running.len() {
        1 => Ok(running.remove(0)),
        n if n > 1 => Err(PlatformError::new(
            ErrorCode::InstanceAmbiguous,
            "multiple running instances; retry with an exact --instance selector",
        )),
        _ => Err(PlatformError::new(
            ErrorCode::InstanceNotFound,
            "no running instance in the selected OCD scope",
        )),
    }
}

/// Return every registered instance with a responsive live control socket.
pub(crate) fn running_instances(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    runtime_root: Option<&Path>,
) -> Result<Vec<InstanceRecord>, PlatformError> {
    let records = registry.list_scope(scope)?;
    let mut running = Vec::new();
    for record in records {
        let id = record.instance_id()?;
        let runtime = runtime_dir_for(record.service_scope, &id, runtime_root)?;
        if probe_status(&runtime)?.is_some() {
            running.push(record);
        }
    }
    Ok(running)
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
    scope: ServiceScope,
    runtime_root: Option<&Path>,
    no_open: bool,
    json: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record =
        resolve_online_instance(config, instance, startup_cwd, registry, scope, runtime_root)?;
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root)?;
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
