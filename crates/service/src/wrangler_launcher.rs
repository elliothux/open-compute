//! Thin project-local Wrangler process launcher.

use crate::auth::resolve_bearer_auth;
use crate::config_load::load_platform_config_from;
use crate::instance_control::{
    CONTROL_SCHEMA_VERSION, GenerationDescriptor, probe_status, runtime_dir_for,
};
use crate::instance_ops::{resolve_online_instance, running_instances};
use crate::instance_registry::{InstanceRecord, InstanceRegistry};
use crate::target_http::{TargetHttp, fetch_capabilities_at};
use crate::target_registry::{TargetRegistry, read_target_token};
use open_compute_core::{
    CloudflareAccountId, ErrorCode, InstanceSelector, PlatformError, SecretString, TargetName,
};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REMOVED_ENVIRONMENT: &[&str] = &[
    "CLOUDFLARE_API_KEY",
    "CLOUDFLARE_EMAIL",
    "CF_API_TOKEN",
    "CF_API_BASE_URL",
    "CF_API_KEY",
    "CF_API_EMAIL",
    "CF_ACCOUNT_ID",
    "CF_EMAIL",
];

/// Complete secret-safe launch plan except for the redacted token wrapper.
#[derive(Clone, Debug)]
pub struct WranglerLaunch {
    /// Exact project-local executable.
    pub executable: PathBuf,
    /// Child working directory.
    pub cwd: PathBuf,
    /// Opaque Wrangler argv passed byte-for-byte.
    pub arguments: Vec<OsString>,
    /// Selected API base URL.
    pub api_base_url: String,
    /// Selected account ID.
    pub account_id: CloudflareAccountId,
    /// Selected target kind for summaries.
    pub target_kind: &'static str,
    /// Selected target or instance name for summaries.
    pub target_name: String,
    /// Detected project-local Wrangler version.
    pub wrangler_version: String,
    /// Exact Wrangler version certified by the selected target.
    pub certified_wrangler_version: String,
    token: SecretString,
}

impl WranglerLaunch {
    /// Replace the current Unix process with the selected Wrangler executable.
    pub fn exec(self, diagnostic: &mut impl Write) -> Result<(), PlatformError> {
        writeln!(
            diagnostic,
            "WRANGLER_TARGET kind={} name={} origin={} account={} wrangler={} certified_wrangler={}",
            self.target_kind,
            self.target_name,
            origin(&self.api_base_url),
            self.account_id,
            self.wrangler_version,
            self.certified_wrangler_version
        )
        .map_err(|_| wrangler_invalid("failed to write the Wrangler target summary"))?;
        let mut command = Command::new(&self.executable);
        command.current_dir(&self.cwd).args(&self.arguments);
        apply_child_environment(&mut command, &self);
        let error = command.exec();
        let _ = error;
        Err(wrangler_invalid(
            "failed to replace the current process with project-local Wrangler",
        ))
    }

    #[cfg(test)]
    fn child_command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.current_dir(&self.cwd).args(&self.arguments);
        apply_child_environment(&mut command, self);
        command
    }
}

/// Resolve the execution target, certified pin, and exact project-local command.
#[allow(
    clippy::too_many_arguments,
    reason = "launcher boundary mirrors the three selectors and injected authorities"
)]
pub async fn prepare_wrangler_launch(
    target: Option<&TargetName>,
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    project: Option<&Path>,
    arguments: &[OsString],
    startup_cwd: &Path,
    instances: &InstanceRegistry,
    targets: &TargetRegistry,
    http: &dyn TargetHttp,
    runtime_root: Option<&Path>,
    diagnostic: &mut impl Write,
) -> Result<WranglerLaunch, PlatformError> {
    if arguments.is_empty() {
        return Err(wrangler_invalid(
            "ocd wrangler requires a Wrangler command or flag",
        ));
    }
    if target.is_some() && (config.is_some() || instance.is_some()) {
        return Err(wrangler_invalid(
            "--target, --instance, and --config are mutually exclusive for ocd wrangler",
        ));
    }
    let execution = match target {
        Some(name) => remote_execution(name, targets)?,
        None => local_execution(
            config,
            instance,
            startup_cwd,
            instances,
            runtime_root,
            diagnostic,
        )?,
    };
    let cwd = resolve_project_directory(project, startup_cwd)?;
    let executable = resolve_project_wrangler(&cwd)?;
    let detected_version = detect_wrangler_version(&executable, &cwd)?;
    let capabilities =
        fetch_capabilities_at(http, &execution.api_base_url, &execution.token).await?;
    if version_major(&detected_version) != version_major(&capabilities.wrangler_version) {
        let _ = writeln!(
            diagnostic,
            "WRANGLER_MAJOR_VERSION_MISMATCH path={} detected={} certified={}",
            executable.display(),
            detected_version,
            capabilities.wrangler_version
        );
    }
    Ok(WranglerLaunch {
        executable,
        cwd,
        arguments: arguments.to_vec(),
        api_base_url: execution.api_base_url,
        account_id: execution.account_id,
        target_kind: execution.kind,
        target_name: execution.name,
        wrangler_version: detected_version,
        certified_wrangler_version: capabilities.wrangler_version,
        token: execution.token,
    })
}

struct ExecutionTarget {
    api_base_url: String,
    account_id: CloudflareAccountId,
    token: SecretString,
    kind: &'static str,
    name: String,
}

fn remote_execution(
    name: &TargetName,
    registry: &TargetRegistry,
) -> Result<ExecutionTarget, PlatformError> {
    let record = registry.get(name)?;
    let token = read_target_token(&record.token_file)?;
    Ok(ExecutionTarget {
        api_base_url: record.api_base_url.to_string(),
        account_id: record.account_id,
        token,
        kind: "target",
        name: record.name.to_string(),
    })
}

fn local_execution(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    runtime_root: Option<&Path>,
    diagnostic: &mut impl Write,
) -> Result<ExecutionTarget, PlatformError> {
    let record =
        match resolve_online_instance(config, instance, startup_cwd, registry, runtime_root) {
            Ok(record) => record,
            Err(error) if error.code() == ErrorCode::InstanceAmbiguous => {
                let candidates = running_instances(registry, runtime_root)?
                    .into_iter()
                    .map(|record| record.instance_id)
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(diagnostic, "WRANGLER_INSTANCE_CANDIDATES {candidates}").map_err(
                    |_| wrangler_invalid("failed to write Wrangler instance diagnostics"),
                )?;
                return Err(error);
            }
            Err(error) if error.code() == ErrorCode::InstanceNotFound => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceNotFound,
                    "no local instance is available; start one or pass --target",
                ));
            }
            Err(error)
                if config.is_none()
                    && instance.is_none()
                    && error.code() == ErrorCode::ConfigPathInvalid =>
            {
                writeln!(
                    diagnostic,
                    "WRANGLER_HINT no local instance is available; start one or pass --target"
                )
                .map_err(|_| wrangler_invalid("failed to write Wrangler instance diagnostics"))?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root);
    let descriptor = probe_status(&runtime)?.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "selected local instance is not running; start it or pass --target",
        )
    })?;
    validate_descriptor(&descriptor, &record)?;
    let loaded = load_platform_config_from(record.config_path(), startup_cwd)?;
    let token = resolve_bearer_auth(&loaded.config.server.deployer_auth)?;
    Ok(ExecutionTarget {
        api_base_url: instance_api_base_url(
            &descriptor,
            loaded.config.server.admin_bind.is_some(),
        )?,
        account_id: descriptor.account_id.parse()?,
        token,
        kind: "instance",
        name: record.instance_id,
    })
}

fn validate_descriptor(
    descriptor: &GenerationDescriptor,
    record: &InstanceRecord,
) -> Result<(), PlatformError> {
    if descriptor.schema_version != CONTROL_SCHEMA_VERSION
        || descriptor.instance_id != record.instance_id
        || descriptor.canonical_config_path != record.canonical_config_path
        || descriptor.service_scope != record.service_scope
        || descriptor.readiness != "ready"
    {
        return Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "selected instance descriptor is not the expected ready generation",
        ));
    }
    Ok(())
}

fn instance_api_base_url(
    descriptor: &GenerationDescriptor,
    distinct_admin_listener: bool,
) -> Result<String, PlatformError> {
    let listener = if distinct_admin_listener {
        descriptor.admin_listener.as_deref()
    } else {
        descriptor.public_listener.as_deref()
    }
    .ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "selected instance does not advertise an admin listener",
        )
    })?;
    let address: SocketAddr = listener.parse().map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "selected instance advertises an invalid listener",
        )
    })?;
    let loopback = if address.ip().is_unspecified() {
        match address.ip() {
            IpAddr::V4(_) => SocketAddr::from(([127, 0, 0, 1], address.port())),
            IpAddr::V6(_) => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], address.port())),
        }
    } else {
        address
    };
    Ok(format!("http://{loopback}/client/v4"))
}

fn resolve_project_directory(
    project: Option<&Path>,
    startup_cwd: &Path,
) -> Result<PathBuf, PlatformError> {
    let candidate = project.map_or_else(
        || startup_cwd.to_path_buf(),
        |value| {
            if value.is_absolute() {
                value.to_path_buf()
            } else {
                startup_cwd.join(value)
            }
        },
    );
    let canonical = fs::canonicalize(candidate)
        .map_err(|_| wrangler_invalid("Wrangler project directory could not be canonicalized"))?;
    if !canonical.is_dir() {
        return Err(wrangler_invalid(
            "Wrangler project path must be a directory",
        ));
    }
    Ok(canonical)
}

fn resolve_project_wrangler(project: &Path) -> Result<PathBuf, PlatformError> {
    let mut directory = project.to_path_buf();
    let device = fs::metadata(&directory)
        .map_err(|_| wrangler_invalid("Wrangler project directory could not be inspected"))?
        .dev();
    loop {
        let candidate = directory.join("node_modules/.bin/wrangler");
        match fs::metadata(&candidate) {
            Ok(meta) => {
                if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                    return Err(wrangler_invalid(
                        "nearest project-local Wrangler is not an executable file",
                    ));
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(wrangler_invalid(
                    "project-local Wrangler could not be inspected",
                ));
            }
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        let parent_meta = fs::metadata(parent)
            .map_err(|_| wrangler_invalid("Wrangler parent directory could not be inspected"))?;
        if parent_meta.dev() != device || parent == directory {
            break;
        }
        directory = parent.to_path_buf();
    }
    Err(wrangler_invalid(
        "project-local Wrangler is missing; install Wrangler in the project",
    ))
}

fn detect_wrangler_version(executable: &Path, cwd: &Path) -> Result<String, PlatformError> {
    let mut command = Command::new(executable);
    command.current_dir(cwd).arg("--version");
    clear_conflicting_environment(&mut command);
    let output = command
        .output()
        .map_err(|_| wrangler_invalid("project-local Wrangler version check could not start"))?;
    if !output.status.success() {
        return Err(wrangler_invalid(
            "project-local Wrangler version check did not exit cleanly",
        ));
    }
    let version = std::str::from_utf8(&output.stdout)
        .map_err(|_| wrangler_invalid("project-local Wrangler version is not UTF-8"))?
        .trim();
    if !valid_version(version) {
        return Err(wrangler_invalid(
            "project-local Wrangler returned an invalid version",
        ));
    }
    Ok(version.to_owned())
}

fn apply_child_environment(command: &mut Command, launch: &WranglerLaunch) {
    clear_conflicting_environment(command);
    command
        .env("CLOUDFLARE_API_BASE_URL", &launch.api_base_url)
        .env("CLOUDFLARE_API_TOKEN", launch.token.expose())
        .env("CLOUDFLARE_ACCOUNT_ID", launch.account_id.as_str())
        .env("WRANGLER_LOG_SANITIZE", "true")
        .env("WRANGLER_SEND_METRICS", "false")
        .env("WRANGLER_SEND_ERROR_REPORTS", "false");
}

fn clear_conflicting_environment(command: &mut Command) {
    command.env_remove("CLOUDFLARE_API_BASE_URL");
    command.env_remove("CLOUDFLARE_API_TOKEN");
    command.env_remove("CLOUDFLARE_ACCOUNT_ID");
    for name in REMOVED_ENVIRONMENT {
        command.env_remove(name);
    }
}

fn valid_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    value.len() <= 32
        && parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.as_bytes().iter().all(u8::is_ascii_digit))
}

fn version_major(value: &str) -> &str {
    let major = value.split_once('.').map_or(value, |(major, _)| major);
    let normalized = major.trim_start_matches('0');
    if normalized.is_empty() {
        "0"
    } else {
        normalized
    }
}

fn origin(api_base_url: &str) -> &str {
    api_base_url
        .strip_suffix("/client/v4")
        .unwrap_or(api_base_url)
}

fn wrangler_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::WranglerInvalid, message)
}

#[cfg(test)]
#[path = "wrangler_launcher_tests.rs"]
mod tests;
