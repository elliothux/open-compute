//! Thin project-local Cf process launcher.

use crate::auth::resolve_bearer_auth;
use crate::config_load::load_platform_config_from;
use crate::instance_control::{
    CONTROL_SCHEMA_VERSION, GenerationDescriptor, probe_status, runtime_dir_for,
};
use crate::instance_ops::{resolve_online_instance, running_instances};
use crate::instance_registry::ServiceScope;
use crate::instance_registry::{InstanceRecord, InstanceRegistry};
use crate::target_http::{TargetHttp, fetch_capabilities_at};
use crate::target_registry::{TargetRegistry, read_target_token};
use open_compute_core::{
    ErrorCode, InstanceId, InstanceSelector, PlatformError, SecretString, TargetName,
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
    "CLOUDFLARE_BASE_URL",
    "CLOUDFLARE_API_USER_SERVICE_KEY",
    "CLOUDFLARE_COMPLIANCE_REGION",
    "CLOUDFLARE_ZONE_ID",
    "CLOUDFLARE_ACCESS_CLIENT_ID",
    "CLOUDFLARE_ACCESS_CLIENT_SECRET",
];

/// Complete secret-safe launch plan except for the redacted token wrapper.
#[derive(Clone, Debug)]
pub struct CfLaunch {
    /// Exact project-local executable.
    pub executable: PathBuf,
    /// Child working directory.
    pub cwd: PathBuf,
    /// Opaque Cf argv passed byte-for-byte.
    pub arguments: Vec<OsString>,
    /// Selected API base URL.
    pub api_base_url: String,
    /// Selected open-compute instance identity.
    pub instance_id: InstanceId,
    /// Selected target kind for summaries.
    pub target_kind: &'static str,
    /// Selected target or instance name for summaries.
    pub target_name: String,
    /// Detected project-local Cf version.
    pub cf_version: String,
    /// Exact Cf version certified by the selected target.
    pub certified_cf_version: String,
    token: SecretString,
}

impl CfLaunch {
    /// Replace the current Unix process with the selected Cf executable.
    pub fn exec(self, diagnostic: &mut impl Write) -> Result<(), PlatformError> {
        writeln!(
            diagnostic,
            "CF_TARGET kind={} name={} origin={} instance_id={} cf={} certified_cf={}",
            self.target_kind,
            self.target_name,
            origin(&self.api_base_url),
            self.instance_id,
            self.cf_version,
            self.certified_cf_version
        )
        .map_err(|_| cf_invalid("failed to write the Cf target summary"))?;
        let mut command = Command::new("node");
        command
            .arg(&self.executable)
            .current_dir(&self.cwd)
            .args(&self.arguments);
        apply_child_environment(&mut command, &self)?;
        let error = command.exec();
        let _ = error;
        Err(cf_invalid(
            "failed to replace the current process with project-local Cf",
        ))
    }

    #[cfg(test)]
    fn child_command(&self) -> Command {
        let mut command = Command::new("node");
        command
            .arg(&self.executable)
            .current_dir(&self.cwd)
            .args(&self.arguments);
        apply_child_environment(&mut command, self).unwrap();
        command
    }
}

/// Resolve the execution target, certified pin, and exact project-local command.
pub async fn prepare_cf_launch(
    target: Option<&TargetName>,
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    project: Option<&Path>,
    arguments: &[OsString],
    startup_cwd: &Path,
    instances: &InstanceRegistry,
    scope: ServiceScope,
    targets: &TargetRegistry,
    http: &dyn TargetHttp,
    runtime_root: Option<&Path>,
    diagnostic: &mut impl Write,
) -> Result<CfLaunch, PlatformError> {
    if arguments.is_empty() {
        return Err(cf_invalid("ocd cf requires a Cf command or flag"));
    }
    if target.is_some() && (config.is_some() || instance.is_some()) {
        return Err(cf_invalid(
            "--target, --instance, and --config are mutually exclusive for ocd cf",
        ));
    }
    let cwd = resolve_project_directory(project, startup_cwd)?;
    crate::cf_project::preflight(&cwd, arguments, diagnostic)?;
    let executable = resolve_project_cf(&cwd)?;
    let detected_version = detect_cf_version(&executable, &cwd)?;
    let execution = match target {
        Some(name) => remote_execution(name, targets)?,
        None => local_execution(
            config,
            instance,
            startup_cwd,
            instances,
            scope,
            runtime_root,
            diagnostic,
        )?,
    };
    let capabilities =
        fetch_capabilities_at(http, &execution.api_base_url, &execution.token).await?;
    if semver::Version::parse(&detected_version).is_ok_and(|v| v.major != 1 || v.minor != 0) {
        let _ = writeln!(
            diagnostic,
            "CF_VERSION_WARNING path={} detected={} certified={}",
            executable.display(),
            detected_version,
            capabilities.cf_version
        );
    }
    Ok(CfLaunch {
        executable,
        cwd,
        arguments: arguments.to_vec(),
        api_base_url: execution.api_base_url,
        instance_id: execution.instance_id,
        target_kind: execution.kind,
        target_name: execution.name,
        cf_version: detected_version,
        certified_cf_version: capabilities.cf_version,
        token: execution.token,
    })
}

struct ExecutionTarget {
    api_base_url: String,
    instance_id: InstanceId,
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
        instance_id: record.instance_id,
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
    scope: ServiceScope,
    runtime_root: Option<&Path>,
    diagnostic: &mut impl Write,
) -> Result<ExecutionTarget, PlatformError> {
    let record =
        match resolve_online_instance(config, instance, startup_cwd, registry, scope, runtime_root)
        {
            Ok(record) => record,
            Err(error) if error.code() == ErrorCode::InstanceAmbiguous => {
                let candidates = running_instances(registry, scope, runtime_root)?
                    .into_iter()
                    .map(|record| record.instance_id)
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(diagnostic, "CF_INSTANCE_CANDIDATES {candidates}")
                    .map_err(|_| cf_invalid("failed to write Cf instance diagnostics"))?;
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
                    "CF_HINT no local instance is available; start one or pass --target"
                )
                .map_err(|_| cf_invalid("failed to write Cf instance diagnostics"))?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
    let id = record.instance_id()?;
    let runtime = runtime_dir_for(record.service_scope, &id, runtime_root)?;
    let descriptor = probe_status(&runtime)?.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceNotFound,
            "selected local instance is not running; start it or pass --target",
        )
    })?;
    validate_descriptor(&descriptor, &record)?;
    let loaded = load_platform_config_from(record.config_path(), startup_cwd)?;
    let token = resolve_bearer_auth(&loaded.config.auth.deployer_auth)?;
    let server = registry.server_config(record.service_scope)?;
    Ok(ExecutionTarget {
        api_base_url: instance_api_base_url(&descriptor, server.admin_bind.is_some())?,
        instance_id: descriptor.instance_id.parse()?,
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
        .map_err(|_| cf_invalid("Cf project directory could not be canonicalized"))?;
    if !canonical.is_dir() {
        return Err(cf_invalid("Cf project path must be a directory"));
    }
    Ok(canonical)
}

fn resolve_project_cf(project: &Path) -> Result<PathBuf, PlatformError> {
    let mut directory = project.to_path_buf();
    let device = fs::metadata(&directory)
        .map_err(|_| cf_invalid("Cf project directory could not be inspected"))?
        .dev();
    loop {
        let candidate = directory.join("node_modules/.bin/cf");
        match fs::metadata(&candidate) {
            Ok(meta) => {
                if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
                    return Err(cf_invalid(
                        "nearest project-local Cf is not an executable file",
                    ));
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(cf_invalid("project-local Cf could not be inspected"));
            }
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        let parent_meta = fs::metadata(parent)
            .map_err(|_| cf_invalid("Cf parent directory could not be inspected"))?;
        if parent_meta.dev() != device || parent == directory {
            break;
        }
        directory = parent.to_path_buf();
    }
    Err(cf_invalid(
        "project-local Cf is missing; install Cf in the project",
    ))
}

fn detect_cf_version(executable: &Path, cwd: &Path) -> Result<String, PlatformError> {
    let node = Command::new("node")
        .arg("--version")
        .output()
        .map_err(|_| cf_invalid("Node.js 22.18 or later is required; install Node explicitly"))?;
    let node_version = std::str::from_utf8(&node.stdout)
        .ok()
        .and_then(|s| semver::Version::parse(s.trim().trim_start_matches('v')).ok());
    if !node.status.success() || node_version.is_none_or(|v| v < semver::Version::new(22, 18, 0)) {
        return Err(cf_invalid("Node.js 22.18 or later is required"));
    }
    let mut command = Command::new("node");
    command.arg(executable).current_dir(cwd).arg("--version");
    clear_conflicting_environment(&mut command);
    command
        .env("CF_SEND_TELEMETRY", "false")
        .env("DO_NOT_TRACK", "1");
    let output = command
        .output()
        .map_err(|_| cf_invalid("project-local Cf version check could not start"))?;
    if !output.status.success() {
        return Err(cf_invalid(
            "project-local Cf version check did not exit cleanly",
        ));
    }
    let version = std::str::from_utf8(&output.stdout)
        .map_err(|_| cf_invalid("project-local Cf version is not UTF-8"))?
        .split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| semver::Version::parse(part).is_ok())
        .ok_or_else(|| cf_invalid("project-local cf returned an invalid version"))?;
    if version.len() > 128 {
        return Err(cf_invalid("project-local Cf returned an invalid version"));
    }
    Ok(version.to_owned())
}

fn apply_child_environment(command: &mut Command, launch: &CfLaunch) -> Result<(), PlatformError> {
    let mut paths = vec![
        launch
            .executable
            .parent()
            .ok_or_else(|| cf_invalid("cf executable has no directory"))?
            .to_path_buf(),
    ];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env(
        "PATH",
        std::env::join_paths(paths)
            .map_err(|_| cf_invalid("project tool directory cannot be added to PATH"))?,
    );
    clear_conflicting_environment(command);
    command
        .env("CLOUDFLARE_API_BASE_URL", &launch.api_base_url)
        .env("CLOUDFLARE_API_TOKEN", launch.token.expose())
        .env("CLOUDFLARE_ACCOUNT_ID", launch.instance_id.as_str())
        .env("WRANGLER_LOG_SANITIZE", "true")
        .env("CF_SEND_TELEMETRY", "false")
        .env("DO_NOT_TRACK", "1");
    Ok(())
}

fn clear_conflicting_environment(command: &mut Command) {
    command.env_remove("CLOUDFLARE_API_BASE_URL");
    command.env_remove("CLOUDFLARE_API_TOKEN");
    command.env_remove("CLOUDFLARE_ACCOUNT_ID");
    for name in REMOVED_ENVIRONMENT {
        command.env_remove(name);
    }
}

fn origin(api_base_url: &str) -> &str {
    api_base_url
        .strip_suffix("/client/v4")
        .unwrap_or(api_base_url)
}

fn cf_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::CfInvalid, message)
}

#[cfg(test)]
#[path = "cf_launcher_tests.rs"]
mod tests;
