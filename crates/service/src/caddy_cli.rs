//! Allowlisted CLI operations for the embedded Caddy binary.

use crate::cli::CaddyCommand;
use crate::config_load::LoadedConfig;
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use open_compute_runtime::{HostProcessSpec, RuntimePackage, run_host_process};
use open_compute_storage::DataDir;
use serde::Deserialize;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
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

/// Run one offline allowlisted command while holding the data-directory lock.
pub(crate) async fn run_offline(
    loaded: &LoadedConfig,
    command: CaddyCommand,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    if matches!(&command, CaddyCommand::Reload) {
        return run_online(loaded, true, out);
    }
    if matches!(&command, CaddyCommand::Status) {
        return run_online(loaded, false, out);
    }
    if matches!(&command, CaddyCommand::Validate)
        && let Some(runtime) = online_runtime(loaded)?
    {
        crate::instance_control::request_caddy_validate(&runtime)?;
        writeln!(out, "CADDY_CONFIG_OK").map_err(|_| io_error())?;
        return Ok(());
    }
    if matches!(
        &command,
        CaddyCommand::ListModules | CaddyCommand::Fmt { .. }
    ) && online_runtime(loaded)?.is_some()
    {
        let package = open_compute_runtime::open_materialized_runtime(
            &loaded.config.data.path.join("runtime"),
        )?;
        return run_read_only(&package, &loaded.config.data.path, command, out).await;
    }
    let data = DataDir::acquire_existing_offline(&loaded.config.data)?;
    let package = open_compute_runtime::materialize_embedded_runtime(&data.runtime_dir())?;
    match command {
        CaddyCommand::ListModules | CaddyCommand::Fmt { .. } => {
            run_read_only(&package, data.root(), command, out).await
        }
        CaddyCommand::Validate => validate(loaded, &data, &package, out).await,
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
    let output = run(package, args, data_root, stdin).await?;
    require_success(&output)?;
    out.write_all(&output.stdout).map_err(|_| io_error())
}

fn run_online(
    loaded: &LoadedConfig,
    reload: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let runtime = online_runtime(loaded)?.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "instance control socket is not available",
        )
    })?;
    let status = crate::instance_control::request_caddy(&runtime, reload)?;
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

fn online_runtime(loaded: &LoadedConfig) -> Result<Option<PathBuf>, PlatformError> {
    let id = open_compute_core::InstanceId::from_canonical_config_path(&loaded.path)?;
    let user = crate::instance_control::runtime_dir_for(
        crate::instance_registry::ServiceScope::User,
        &id,
        None,
    );
    let system = crate::instance_control::runtime_dir_for(
        crate::instance_registry::ServiceScope::System,
        &id,
        None,
    );
    Ok([user, system]
        .into_iter()
        .find(|runtime| runtime.join("control.sock").exists()))
}

async fn validate(
    loaded: &LoadedConfig,
    data: &DataDir,
    package: &RuntimePackage,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let config = loaded.config.public_gateway.as_ref().ok_or_else(|| {
        PlatformError::new(ErrorCode::ConfigInvalid, "public gateway is not configured")
    })?;
    let gateway = data.prepare_gateway_dir()?;
    let candidate = gateway
        .join("config-state")
        .join(format!("validate-{}", uuid::Uuid::now_v7()));
    open_compute_storage::ensure_dir_secure(&candidate)?;
    let result = async {
        crate::gateway_caddyfile::write_managed(
            config,
            &candidate,
            &gateway.join("run/gw.sock"),
            &gateway.join("run/dns.sock"),
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
            &candidate,
            Vec::new(),
        )
        .await?;
        require_success(&output)?;
        writeln!(out, "CADDY_CONFIG_OK").map_err(|_| io_error())
    }
    .await;
    let cleanup = std::fs::remove_dir_all(&candidate).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "failed to remove Caddy validation workspace",
        )
    });
    result.and(cleanup)
}

async fn run(
    package: &RuntimePackage,
    args: Vec<OsString>,
    cwd: &Path,
    stdin: Vec<u8>,
) -> Result<open_compute_runtime::BoundedOutput, PlatformError> {
    let (image, _) = package.caddy()?;
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
        },
    )
    .await
}

fn tool_environment(cwd: &Path) -> Vec<(OsString, OsString)> {
    vec![
        ("HOME".into(), cwd.as_os_str().to_owned()),
        ("XDG_CONFIG_HOME".into(), cwd.as_os_str().to_owned()),
        ("XDG_DATA_HOME".into(), cwd.as_os_str().to_owned()),
    ]
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
