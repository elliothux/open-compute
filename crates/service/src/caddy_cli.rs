//! Allowlisted CLI operations for the embedded Caddy binary.

use crate::cli::CaddyCommand;
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use open_compute_core::{DaemonGatewayConfig, ErrorCode, PlatformError, Redactor};
use open_compute_runtime::{HostProcessLease, HostProcessSpec, RuntimePackage, run_host_process};
use serde::Deserialize;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const TOOL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaddyLock {
    release: String,
    expected_version_output: String,
    targets: serde_json::Value,
    source: serde_json::Value,
    schema_version: u32,
}

/// Print the embedded manifest version without materializing the executable.
pub(crate) fn write_version(out: &mut impl Write) -> Result<(), PlatformError> {
    let lock = lock()?;
    writeln!(out, "{}", lock.expected_version_output).map_err(|_| io_error())?;
    writeln!(out, "pin {}", lock.release).map_err(|_| io_error())
}

/// Run one allowlisted command against the selected daemon scope.
pub(crate) async fn run_offline(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    command: CaddyCommand,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let root = registry.root_for(scope);
    if matches!(&command, CaddyCommand::Reload) {
        return run_online(root, true, out);
    }
    if matches!(&command, CaddyCommand::Status) {
        return run_online(root, false, out);
    }
    if matches!(&command, CaddyCommand::Validate) && root.join("run/control.sock").exists() {
        require_gateway_response(crate::run::daemon_control::exchange(
            root,
            &crate::run::daemon_control::ControlRequest::CaddyValidate,
        )?)?;
        writeln!(out, "CADDY_CONFIG_OK").map_err(|_| io_error())?;
        return Ok(());
    }
    if matches!(
        &command,
        CaddyCommand::ListModules | CaddyCommand::Fmt { .. }
    ) && root.join("run/control.sock").exists()
    {
        let package = open_compute_runtime::open_materialized_runtime(&root.join("cache"))?;
        return run_read_only(&package, root, command, out).await;
    }
    let _lock = crate::run::DaemonLock::acquire(root)?;
    let cache_dir = root.join("cache");
    open_compute_storage::ensure_dir_secure(&cache_dir)?;
    let package = open_compute_runtime::materialize_embedded_runtime(&cache_dir)?;
    match command {
        CaddyCommand::ListModules | CaddyCommand::Fmt { .. } => {
            run_read_only(&package, root, command, out).await
        }
        CaddyCommand::Validate => {
            let shared = registry.gateway_config(scope)?.ok_or_else(|| {
                PlatformError::new(ErrorCode::ConfigInvalid, "shared Gateway is not configured")
            })?;
            let domains = registry
                .list_scope(scope)?
                .into_iter()
                .filter_map(|record| record.public_base_domain)
                .collect::<Vec<_>>();
            validate(root, &shared, &domains, &package, out).await
        }
        CaddyCommand::Reload | CaddyCommand::Status => unreachable!(),
        CaddyCommand::Version => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "invalid offline Caddy command",
        )),
    }
}

async fn run_read_only(
    package: &RuntimePackage,
    data_root: &Path,
    command: CaddyCommand,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let (args, stdin) = match command {
        CaddyCommand::ListModules => (vec!["list-modules".into()], Vec::new()),
        CaddyCommand::Fmt { file } => {
            let cwd = std::env::current_dir().map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "startup working directory is unavailable",
                )
            })?;
            let file = crate::config_load::lexical_absolute(&cwd, &file)?;
            open_compute_storage::validate_owned_file(&file, false)?;
            let source = std::fs::read(file).map_err(|_| {
                PlatformError::new(ErrorCode::ConfigPathInvalid, "failed to read Caddyfile")
            })?;
            (vec!["fmt".into(), "-".into()], source)
        }
        _ => unreachable!(),
    };
    let tmp_root = data_root.join("tmp");
    open_compute_storage::ensure_dir_secure(&tmp_root)?;
    let (_, digest) = package.caddy()?;
    crate::task_workspace::recover(
        &tmp_root,
        &["caddy-tool-", "caddy-validate-"],
        "tool.lease",
        digest,
    )?;
    let workspace = crate::task_workspace::create(&tmp_root, "caddy-tool-")?;
    let result = run(package, args, workspace.path(), stdin).await;
    let completed = crate::task_workspace::mark_completed(workspace.path());
    let cleanup = workspace
        .close()
        .map_err(|_| tool_error("failed to remove private Caddy workspace"));
    let output = result?;
    completed?;
    cleanup?;
    require_success(&output)?;
    out.write_all(&output.stdout).map_err(|_| io_error())
}

fn run_online(root: &Path, reload: bool, out: &mut impl Write) -> Result<(), PlatformError> {
    let request = if reload {
        crate::run::daemon_control::ControlRequest::CaddyReload
    } else {
        crate::run::daemon_control::ControlRequest::CaddyStatus
    };
    let status = require_gateway_response(crate::run::daemon_control::exchange(root, &request)?)?;
    let command = if reload {
        "CADDY_RELOAD_OK"
    } else {
        "CADDY_STATUS"
    };
    writeln!(
        out,
        "{command} child_pid={} tls_ready={} dns={} config_sha256={} last_reload={} last_error={}",
        status
            .child_pid
            .map_or_else(|| "-".to_owned(), |pid| pid.to_string()),
        status.tls_ready,
        status.dns,
        status.config_sha256.as_deref().unwrap_or("-"),
        status.last_reload,
        status.last_error.as_deref().unwrap_or("-"),
    )
    .map_err(|_| io_error())
}

async fn validate(
    root: &Path,
    shared: &DaemonGatewayConfig,
    domains: &[String],
    package: &RuntimePackage,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let tmp_dir = root.join("tmp");
    open_compute_storage::ensure_dir_secure(&tmp_dir)?;
    let (_, digest) = package.caddy()?;
    crate::task_workspace::recover(
        &tmp_dir,
        &["caddy-tool-", "caddy-validate-"],
        "tool.lease",
        digest,
    )?;
    let socket_dir = root.join("run/gateway");
    let workspace = crate::task_workspace::create(&tmp_dir, "caddy-validate-")?;
    let candidate = workspace.path();
    let result = async {
        crate::gateway_caddyfile::write_managed(
            shared,
            domains,
            candidate,
            &socket_dir.join("admin.sock"),
            &socket_dir.join("upstream.sock"),
            &socket_dir.join("dns.sock"),
        )?;
        let output = run(
            package,
            vec![
                "adapt".into(),
                "--config".into(),
                candidate.join("Caddyfile").into_os_string(),
                "--adapter".into(),
                "caddyfile".into(),
                "--validate".into(),
            ],
            candidate,
            Vec::new(),
        )
        .await?;
        require_success(&output)?;
        writeln!(out, "CADDY_CONFIG_OK").map_err(|_| io_error())
    }
    .await;
    let completed = crate::task_workspace::mark_completed(workspace.path());
    let cleanup = workspace.close().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "failed to remove Caddy validation workspace",
        )
    });
    result.and(completed).and(cleanup)
}

fn require_gateway_response(
    response: crate::run::daemon_control::ControlResponse,
) -> Result<crate::gateway_control::GatewayStatus, PlatformError> {
    if !response.ok {
        return Err(tool_error("shared Gateway operation failed"));
    }
    response
        .gateway_status
        .ok_or_else(|| tool_error("shared Gateway response was incomplete"))
}

async fn run(
    package: &RuntimePackage,
    args: Vec<OsString>,
    cwd: &Path,
    stdin: Vec<u8>,
) -> Result<open_compute_runtime::BoundedOutput, PlatformError> {
    let (image, digest) = package.caddy()?;
    open_compute_storage::ensure_dir_secure(&cwd.join("tmp"))?;
    run_host_process(
        &image,
        HostProcessSpec {
            args,
            environment: tool_environment(cwd),
            working_directory: cwd.to_owned(),
            stdin,
            deadline: TOOL_DEADLINE,
            max_stdout: MAX_OUTPUT,
            max_stderr: 64 * 1024,
            redactor: Redactor::new(),
            lease: Some(HostProcessLease {
                path: cwd.join("tool.lease"),
                binary_sha256: digest.to_owned(),
            }),
        },
    )
    .await
}

fn tool_environment(cwd: &Path) -> Vec<(OsString, OsString)> {
    let tmp = cwd.join("tmp").into_os_string();
    vec![
        ("HOME".into(), tmp.clone()),
        ("XDG_CONFIG_HOME".into(), tmp.clone()),
        ("XDG_DATA_HOME".into(), tmp.clone()),
        ("XDG_CACHE_HOME".into(), tmp.clone()),
        ("TMPDIR".into(), tmp.clone()),
        ("TMP".into(), tmp.clone()),
        ("TEMP".into(), tmp),
    ]
}

#[cfg(test)]
#[test]
fn caddy_tool_environment_is_confined_to_the_selected_owner() {
    let owner = Path::new("/owned/ocd");
    let expected = owner.join("tmp").into_os_string();
    let environment = tool_environment(owner);
    assert_eq!(environment.len(), 7);
    assert!(environment.iter().all(|(_, value)| *value == expected));
}

fn require_success(output: &open_compute_runtime::BoundedOutput) -> Result<(), PlatformError> {
    if output.timed_out || output.stdout_overflow || output.stderr_overflow {
        return Err(tool_error("embedded Caddy command exceeded its bound"));
    }
    if output.status.is_some_and(|status| status.success()) {
        return Ok(());
    }
    Err(tool_error("embedded Caddy command failed"))
}

fn lock() -> Result<CaddyLock, PlatformError> {
    let lock: CaddyLock = serde_json::from_slice(open_compute_runtime::embedded_caddy_lock()?)
        .map_err(|_| tool_error("embedded Caddy lock is invalid"))?;
    if lock.schema_version != 1 || !lock.targets.is_object() || !lock.source.is_object() {
        return Err(tool_error("embedded Caddy lock is invalid"));
    }
    Ok(lock)
}

fn tool_error(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeInvalid, message)
}

fn io_error() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    fn output(status: i32) -> open_compute_runtime::BoundedOutput {
        open_compute_runtime::BoundedOutput {
            status: Some(std::process::ExitStatus::from_raw(status)),
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
            stdout_overflow: false,
            stderr_overflow: false,
            stdin_error: false,
            pid: None,
        }
    }

    #[test]
    fn command_status_must_be_successful_and_bounded() {
        assert!(require_success(&output(0)).is_ok());
        assert!(require_success(&output(1)).is_err());
        let mut exceeded = output(0);
        exceeded.timed_out = true;
        assert!(require_success(&exceeded).is_err());
    }
}
