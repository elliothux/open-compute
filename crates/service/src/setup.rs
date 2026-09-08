//! Interactive and `--yes` first-host setup for `ocd`.

use crate::config_load::{lexical_absolute, load_platform_config_from};
use crate::instance_ops::{INSTANCE_READY_TIMEOUT, wait_until_instance_ready};
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::service_manager::ServiceManager;
use open_compute_core::config::{
    DashboardConfig, ObjectStorageConfig, PlatformConfig, SecretReference,
};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::ensure_dir_secure;
use rand::TryRngCore;
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

const DEFAULT_CONFIG: &str = include_str!("../../../share/default-config.toml");
const TOKEN_BYTES: usize = 32;
const SYSTEM_CONFIG_PARENT: &str = "/etc/open-compute";
const SYSTEM_DATA_DIR: &str = "/var/lib/open-compute";
const USER_DATA_REL: &str = ".data/open-compute";

/// Injectable filesystem and registry roots for setup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupRoots {
    /// Parent directory that will hold the configuration file.
    pub config_parent: PathBuf,
    /// Absolute data directory written into the generated configuration.
    pub data_dir: PathBuf,
    /// Absolute directory for generated Bearer token files.
    pub secrets_dir: PathBuf,
    /// System-scope instance registry root.
    pub system_registry_root: PathBuf,
    /// User-scope instance registry root.
    pub user_registry_root: PathBuf,
}

impl SetupRoots {
    /// Production roots for system or project-local setup.
    ///
    /// Without `--config`, returns system defaults under `/etc/open-compute` and
    /// `/var/lib/open-compute`. With `--config`, returns user-scope roots whose
    /// data directory is `<startup_cwd>/.data/open-compute`.
    pub fn production(
        startup_cwd: &Path,
        config: Option<&Path>,
        system: bool,
    ) -> Result<(Self, PathBuf, ServiceScope), PlatformError> {
        if system && config.is_some() {
            return Err(PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "`ocd setup --system` cannot be combined with `--config`",
            ));
        }
        let registry = InstanceRegistry::production()?;
        let system_registry_root = registry.root_for(ServiceScope::System).to_owned();
        let user_registry_root = registry.root_for(ServiceScope::User).to_owned();
        if let Some(config) = config {
            let config_path = lexical_absolute(startup_cwd, config)?;
            let config_parent = config_path
                .parent()
                .ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::ConfigPathInvalid,
                        "setup --config path must name a regular file",
                    )
                })?
                .to_owned();
            let data_dir = lexical_absolute(startup_cwd, Path::new(USER_DATA_REL))?;
            Ok((
                Self {
                    config_parent,
                    secrets_dir: data_dir.join("secrets"),
                    data_dir,
                    system_registry_root,
                    user_registry_root,
                },
                config_path,
                ServiceScope::User,
            ))
        } else {
            let config_parent = PathBuf::from(SYSTEM_CONFIG_PARENT);
            let data_dir = PathBuf::from(SYSTEM_DATA_DIR);
            Ok((
                Self {
                    config_parent: config_parent.clone(),
                    secrets_dir: data_dir.join("secrets"),
                    data_dir,
                    system_registry_root,
                    user_registry_root,
                },
                config_parent.join("config.toml"),
                ServiceScope::System,
            ))
        }
    }
}

/// Options for [`run_setup`].
#[derive(Clone, Debug)]
pub struct SetupOptions {
    /// Optional exact configuration path to create.
    pub config: Option<PathBuf>,
    /// Force system-scope defaults when no `--config` is supplied.
    pub system: bool,
    /// Apply recommended defaults without prompts.
    pub yes: bool,
    /// Filesystem and registry roots (injectable in tests).
    pub roots: SetupRoots,
    /// Resolved configuration path when roots were constructed via [`SetupRoots::production`].
    pub config_path: PathBuf,
    /// Service scope for registration.
    pub scope: ServiceScope,
}

/// Run first-host setup: exclusive-create secrets and config, register, and start.
pub fn run_setup(
    options: &SetupOptions,
    startup_cwd: &Path,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let plan = if options.yes {
        plan_yes(options)
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "non-interactive setup requires `--yes`; retry with `ocd setup --yes`",
            ));
        }
        plan_interactive(
            options,
            startup_cwd,
            &mut std::io::stdin().lock(),
            &mut std::io::stderr(),
        )?
    };
    execute_plan(&plan, startup_cwd, manager, out)
}

fn plan_yes(options: &SetupOptions) -> SetupPlan {
    SetupPlan {
        scope: options.scope,
        config_path: options.config_path.clone(),
        data_dir: options.roots.data_dir.clone(),
        secrets_dir: options.roots.secrets_dir.clone(),
        public_bind: "127.0.0.1:8787".to_owned(),
        dashboard_enabled: true,
        start_service: true,
        system_registry_root: options.roots.system_registry_root.clone(),
        user_registry_root: options.roots.user_registry_root.clone(),
    }
}

fn plan_interactive(
    options: &SetupOptions,
    startup_cwd: &Path,
    input: &mut dyn std::io::BufRead,
    prompt_out: &mut dyn Write,
) -> Result<SetupPlan, PlatformError> {
    writeln_prompt(
        prompt_out,
        "Interactive setup: press Enter to accept the default shown in [brackets].",
    )?;

    let default_system = matches!(options.scope, ServiceScope::System) || options.system;
    let scope = match prompt_line(
        input,
        prompt_out,
        &format!(
            "Service scope (system/user) [{}]",
            if default_system { "system" } else { "user" }
        ),
        if default_system { "system" } else { "user" },
    )?
    .as_str()
    {
        "system" => ServiceScope::System,
        "user" => ServiceScope::User,
        _ => {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "unsupported service scope; expected system or user",
            ));
        }
    };

    let (default_config, default_data, default_secrets) = match scope {
        ServiceScope::System => (
            PathBuf::from(SYSTEM_CONFIG_PARENT).join("config.toml"),
            PathBuf::from(SYSTEM_DATA_DIR),
            PathBuf::from(SYSTEM_DATA_DIR).join("secrets"),
        ),
        ServiceScope::User => {
            let config = if options.config_path.starts_with(SYSTEM_CONFIG_PARENT) {
                lexical_absolute(startup_cwd, Path::new("compute.toml"))?
            } else {
                options.config_path.clone()
            };
            let data = lexical_absolute(startup_cwd, Path::new(USER_DATA_REL))?;
            (config, data.clone(), data.join("secrets"))
        }
    };

    let config_path = PathBuf::from(prompt_line(
        input,
        prompt_out,
        &format!("Config path [{}]", default_config.display()),
        &default_config.to_string_lossy(),
    )?);
    let config_path = lexical_absolute(startup_cwd, &config_path)?;
    let data_dir = PathBuf::from(prompt_line(
        input,
        prompt_out,
        &format!("Data directory [{}]", default_data.display()),
        &default_data.to_string_lossy(),
    )?);
    let data_dir = lexical_absolute(startup_cwd, &data_dir)?;
    let secrets_dir = data_dir.join("secrets");
    let _ = default_secrets;

    let public_bind = prompt_line(
        input,
        prompt_out,
        "Public/admin listener [127.0.0.1:8787]",
        "127.0.0.1:8787",
    )?;
    let backend = prompt_line(
        input,
        prompt_out,
        "Object backend (local/s3) [local]",
        "local",
    )?;
    if backend != "local" {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "interactive setup currently supports only the local object backend; pass `--yes` for local defaults or write a config manually for S3",
        ));
    }
    let dashboard_enabled = prompt_bool(input, prompt_out, "Enable Dashboard? [Y/n]", true)?;
    let start_service = prompt_bool(
        input,
        prompt_out,
        "Register, enable, and start the service now? [Y/n]",
        true,
    )?;

    Ok(SetupPlan {
        scope,
        config_path,
        data_dir,
        secrets_dir,
        public_bind,
        dashboard_enabled,
        start_service,
        system_registry_root: options.roots.system_registry_root.clone(),
        user_registry_root: options.roots.user_registry_root.clone(),
    })
}

#[derive(Clone, Debug)]
struct SetupPlan {
    scope: ServiceScope,
    config_path: PathBuf,
    data_dir: PathBuf,
    secrets_dir: PathBuf,
    public_bind: String,
    dashboard_enabled: bool,
    start_service: bool,
    system_registry_root: PathBuf,
    user_registry_root: PathBuf,
}

fn execute_plan(
    plan: &SetupPlan,
    startup_cwd: &Path,
    manager: &dyn ServiceManager,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let service_account = match plan.scope {
        ServiceScope::System => Some(crate::service_manager::system_service_account()?),
        ServiceScope::User => None,
    };
    let config_parent = plan.config_path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "setup config path must name a regular file",
        )
    })?;
    let admin_secret = plan.secrets_dir.join("admin.token");
    let deployer_secret = plan.secrets_dir.join("deployer.token");
    let read_only_secret = plan.secrets_dir.join("read-only.token");
    let master_key_file = plan.data_dir.join("keys/master.key");
    let objects_dir = plan.data_dir.join("objects");
    let keys_dir = plan.data_dir.join("keys");

    refuse_existing(&plan.config_path, "configuration file")?;
    refuse_existing(&admin_secret, "admin token file")?;
    refuse_existing(&deployer_secret, "deployer token file")?;
    refuse_existing(&read_only_secret, "read-only token file")?;
    refuse_existing(&master_key_file, "master key file")?;

    writeln!(
        out,
        "SETUP_PLAN scope={} config={} data_dir={} listener={} dashboard={} start={}",
        plan.scope.as_str(),
        plan.config_path.display(),
        plan.data_dir.display(),
        plan.public_bind,
        plan.dashboard_enabled,
        plan.start_service
    )
    .map_err(|_| io_failed())?;

    ensure_dir_tree(config_parent, plan.scope)?;
    ensure_dir_tree(&plan.data_dir, plan.scope)?;
    ensure_dir_tree(&plan.secrets_dir, plan.scope)?;
    ensure_dir_tree(&keys_dir, plan.scope)?;
    ensure_dir_tree(&objects_dir, plan.scope)?;

    let staging_name = format!(".ocd-setup-staging-{}", Uuid::now_v7().as_hyphenated());
    let staging_dir = config_parent.join(&staging_name);
    if let Err(err) = prepare_staging(
        plan,
        &staging_dir,
        &admin_secret,
        &deployer_secret,
        &read_only_secret,
        &master_key_file,
        &objects_dir,
        startup_cwd,
    ) {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(err);
    }

    let staging_secrets = staging_dir.join("secrets");
    let published = match publish_exclusive(
        plan,
        &staging_dir,
        &staging_secrets,
        &admin_secret,
        &deployer_secret,
        &read_only_secret,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(error);
        }
    };
    let _ = fs::remove_dir_all(&staging_dir);

    if !plan.start_service {
        let prepared = (|| {
            if let Some(account) = &service_account {
                assign_system_ownership(plan, account)?;
            }
            load_platform_config_from(&plan.config_path, startup_cwd)?;
            Ok::<(), PlatformError>(())
        })();
        if let Err(error) = prepared {
            if remove_published_files(&published).is_err() {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "setup failed before registration and rollback was incomplete",
                ));
            }
            return Err(error);
        }
        writeln!(
            out,
            "SETUP_OK config={} scope={} registered=false",
            plan.config_path.display(),
            plan.scope.as_str(),
        )
        .map_err(|_| io_failed())?;
        writeln!(
            out,
            "Next: run `ocd start --config {}` when this instance should be registered.",
            plan.config_path.display(),
        )
        .map_err(|_| io_failed())?;
        return Ok(());
    }

    let registry = InstanceRegistry::with_roots(
        plan.system_registry_root.clone(),
        plan.user_registry_root.clone(),
    );
    let mut registered = None;
    let mut service_installed = false;
    let prepared = (|| {
        if let Some(account) = &service_account {
            assign_system_ownership(plan, account)?;
        }
        let loaded = load_platform_config_from(&plan.config_path, startup_cwd)?;
        if registry
            .list()?
            .iter()
            .any(|record| record.canonical_config_path == loaded.path.to_string_lossy())
        {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "refusing to reuse an existing instance registration during setup",
            ));
        }
        let record = registry.register_with_service_user(
            &loaded.path,
            plan.scope,
            service_account
                .as_ref()
                .map(|account| account.name.as_str()),
            SystemTime::now(),
        )?;
        registered = Some(record.clone());
        let ocd = std::env::current_exe().map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "failed to resolve the current ocd executable path",
            )
        })?;
        service_installed = true;
        manager.install(&record, &ocd)?;
        manager.enable(&record)?;
        Ok(record)
    })();
    let record = match prepared {
        Ok(record) => record,
        Err(error) => {
            if rollback_pre_start(
                &published,
                registered.as_ref(),
                service_installed,
                &registry,
                manager,
            )
            .is_err()
            {
                return Err(PlatformError::new(
                    ErrorCode::PlatformUnavailable,
                    "setup failed before activation and rollback was incomplete",
                ));
            }
            return Err(error);
        }
    };

    // Once activation is attempted, retain the complete registered install on
    // failure. The daemon may have initialized authority in data-dir, so an
    // automatic destructive rollback would be unsafe; `ocd start` can retry it.
    manager.start(&record)?;
    wait_until_instance_ready(
        &record,
        manager.readiness_runtime_root().as_deref(),
        INSTANCE_READY_TIMEOUT,
    )?;

    writeln!(
        out,
        "SETUP_OK instance={} config={} scope={}",
        record.instance_id,
        record.canonical_config_path,
        plan.scope.as_str()
    )
    .map_err(|_| io_failed())?;
    writeln!(
        out,
        "Next: run `ocd dashboard --instance {}`.",
        record.instance_id
    )
    .map_err(|_| io_failed())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_staging(
    plan: &SetupPlan,
    staging_dir: &Path,
    admin_secret: &Path,
    deployer_secret: &Path,
    read_only_secret: &Path,
    master_key_file: &Path,
    objects_dir: &Path,
    startup_cwd: &Path,
) -> Result<(), PlatformError> {
    create_dir_mapped(staging_dir, plan.scope)?;
    let staging_secrets = staging_dir.join("secrets");
    create_dir_mapped(&staging_secrets, plan.scope)?;

    let (admin, deployer, read_only) = generate_distinct_tokens()?;
    exclusive_write_secret(&staging_secrets.join("admin.token"), &admin, plan.scope)?;
    exclusive_write_secret(
        &staging_secrets.join("deployer.token"),
        &deployer,
        plan.scope,
    )?;
    exclusive_write_secret(
        &staging_secrets.join("read-only.token"),
        &read_only,
        plan.scope,
    )?;

    let mut config = PlatformConfig::from_toml_str(DEFAULT_CONFIG)?;
    config.server.public_bind = plan.public_bind.clone();
    config.server.admin_auth = SecretReference {
        env: None,
        file: Some(admin_secret.to_owned()),
    };
    config.server.deployer_auth = SecretReference {
        env: None,
        file: Some(deployer_secret.to_owned()),
    };
    config.server.read_only_auth = SecretReference {
        env: None,
        file: Some(read_only_secret.to_owned()),
    };
    config.data.path = plan.data_dir.clone();
    config.data.master_key_file = master_key_file.to_owned();
    config.data.master_key_env = None;
    let ObjectStorageConfig::Local(local) = &mut config.object_storage else {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "embedded default config must use local object storage",
        ));
    };
    local.path = objects_dir.to_owned();
    config.dashboard = DashboardConfig {
        enabled: plan.dashboard_enabled,
    };
    config.validate()?;

    let text = format!(
        "# Generated by `ocd setup`. Secrets are file references only.\n{}",
        toml::to_string_pretty(&config).map_err(|_| PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to serialize setup configuration",
        ))?
    );
    let staging_config = staging_dir.join(plan.config_path.file_name().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "setup config path must name a regular file",
        )
    })?);
    exclusive_write_bytes(&staging_config, text.as_bytes(), 0o600, plan.scope)?;
    let _ = load_platform_config_from(&staging_config, startup_cwd)?;
    Ok(())
}

fn publish_exclusive(
    plan: &SetupPlan,
    staging_dir: &Path,
    staging_secrets: &Path,
    admin_secret: &Path,
    deployer_secret: &Path,
    read_only_secret: &Path,
) -> Result<Vec<PathBuf>, PlatformError> {
    let mut published = Vec::new();
    let result = (|| {
        publish_file(
            &staging_secrets.join("admin.token"),
            admin_secret,
            plan.scope,
        )?;
        published.push(admin_secret.to_owned());
        publish_file(
            &staging_secrets.join("deployer.token"),
            deployer_secret,
            plan.scope,
        )?;
        published.push(deployer_secret.to_owned());
        publish_file(
            &staging_secrets.join("read-only.token"),
            read_only_secret,
            plan.scope,
        )?;
        published.push(read_only_secret.to_owned());
        let staging_config = staging_dir.join(plan.config_path.file_name().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "setup config path must name a regular file",
            )
        })?);
        publish_file(&staging_config, &plan.config_path, plan.scope)?;
        published.push(plan.config_path.clone());
        Ok(())
    })();
    if let Err(error) = result {
        if remove_published_files(&published).is_err() {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "setup file publication failed and rollback was incomplete",
            ));
        }
        return Err(error);
    }
    Ok(published)
}

fn rollback_pre_start(
    published: &[PathBuf],
    record: Option<&crate::instance_registry::InstanceRecord>,
    service_installed: bool,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
) -> Result<(), PlatformError> {
    if let Some(record) = record {
        if service_installed {
            manager.uninstall(record)?;
        }
        let selector = record.instance_id.parse()?;
        registry.remove(&selector)?;
    }
    remove_published_files(published)
}

fn remove_published_files(paths: &[PathBuf]) -> Result<(), PlatformError> {
    for path in paths.iter().rev() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "failed to remove a setup file during rollback",
                ));
            }
        }
    }
    Ok(())
}

fn assign_system_ownership(
    plan: &SetupPlan,
    account: &crate::service_manager::SystemServiceAccount,
) -> Result<(), PlatformError> {
    assign_path_ownership(&plan.data_dir, account, true)?;
    fs::set_permissions(&plan.config_path, fs::Permissions::from_mode(0o644)).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to make the system configuration readable by the service account",
        )
    })?;
    if let Some(parent) = plan.config_path.parent() {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to make the system configuration directory traversable",
            )
        })?;
    }
    Ok(())
}

fn assign_path_ownership(
    path: &Path,
    account: &crate::service_manager::SystemServiceAccount,
    recursive: bool,
) -> Result<(), PlatformError> {
    let owner = format!("{}:{}", account.uid, account.gid);
    let path_text = path.to_string_lossy();
    let mut command = std::process::Command::new("chown");
    if recursive {
        command.arg("-R");
    }
    let status = command.args([owner.as_str(), path_text.as_ref()]).status();
    if status.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to assign setup files to the non-root service account",
        ))
    }
}

fn publish_file(from: &Path, to: &Path, scope: ServiceScope) -> Result<(), PlatformError> {
    refuse_existing(to, "setup target")?;
    fs::hard_link(from, to)
        .or_else(|_| fs::copy(from, to).map(|_| ()))
        .map_err(|err| {
            map_privilege(
                &err,
                scope,
                "failed to publish a setup file to its final path",
            )
        })?;
    let meta = fs::symlink_metadata(to)
        .map_err(|err| map_privilege(&err, scope, "failed to inspect a published setup file"))?;
    if meta.permissions().mode() & 0o777 != 0o600 {
        fs::set_permissions(to, fs::Permissions::from_mode(0o600)).map_err(|err| {
            map_privilege(
                &err,
                scope,
                "failed to set mode 0600 on a published setup file",
            )
        })?;
    }
    let _ = fs::remove_file(from);
    Ok(())
}

fn generate_distinct_tokens() -> Result<(String, String, String), PlatformError> {
    let mut tokens = Vec::with_capacity(3);
    while tokens.len() < 3 {
        let candidate = random_token()?;
        if !tokens.iter().any(|existing| existing == &candidate) {
            tokens.push(candidate);
        }
    }
    Ok((tokens.remove(0), tokens.remove(0), tokens.remove(0)))
}

fn random_token() -> Result<String, PlatformError> {
    let mut buf = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.try_fill_bytes(&mut buf).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "failed to generate setup Bearer token material",
        )
    })?;
    Ok(hex::encode(buf))
}

fn exclusive_write_secret(
    path: &Path,
    value: &str,
    scope: ServiceScope,
) -> Result<(), PlatformError> {
    let mut body = value.as_bytes().to_vec();
    body.push(b'\n');
    exclusive_write_bytes(path, &body, 0o600, scope)
}

fn exclusive_write_bytes(
    path: &Path,
    contents: &[u8],
    mode: u32,
    scope: ServiceScope,
) -> Result<(), PlatformError> {
    refuse_existing(path, "setup target")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|err| map_privilege(&err, scope, "failed to exclusive-create a setup file"))?;
    file.write_all(contents)
        .map_err(|err| map_privilege(&err, scope, "failed to write a setup file"))?;
    file.sync_all()
        .map_err(|err| map_privilege(&err, scope, "failed to fsync a setup file"))?;
    Ok(())
}

fn ensure_dir_tree(path: &Path, scope: ServiceScope) -> Result<(), PlatformError> {
    if path.as_os_str().is_empty() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "setup directory path is empty",
        ));
    }
    let mut built = PathBuf::new();
    for component in path.components() {
        built.push(component.as_os_str());
        if matches!(
            component,
            std::path::Component::RootDir | std::path::Component::Prefix(_)
        ) {
            continue;
        }
        if built.exists() {
            continue;
        }
        create_dir_mapped(&built, scope)?;
    }
    if path.exists() {
        ensure_dir_secure(path).map_err(|err| {
            if matches!(scope, ServiceScope::System) {
                PlatformError::new(
                    err.code(),
                    "insufficient privileges or insecure setup directory; retry with `sudo ocd setup --yes` when privilege is required",
                )
            } else {
                err
            }
        })?;
    }
    Ok(())
}

fn create_dir_mapped(path: &Path, scope: ServiceScope) -> Result<(), PlatformError> {
    match fs::create_dir(path) {
        Ok(()) => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
                map_privilege(&err, scope, "failed to set mode 0700 on a setup directory")
            })?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(map_privilege(
            &err,
            scope,
            "failed to create a setup directory",
        )),
    }
}

fn refuse_existing(path: &Path, _label: &str) -> Result<(), PlatformError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "refusing to overwrite an existing setup target",
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to inspect a setup target path",
        )),
    }
}

fn map_privilege(
    err: &std::io::Error,
    scope: ServiceScope,
    fallback: &'static str,
) -> PlatformError {
    if err.kind() == std::io::ErrorKind::PermissionDenied && matches!(scope, ServiceScope::System) {
        return PlatformError::new(
            ErrorCode::PathInvalid,
            "insufficient privileges to write system setup paths; retry with `sudo ocd setup --yes`",
        );
    }
    if err.kind() == std::io::ErrorKind::AlreadyExists {
        return PlatformError::new(
            ErrorCode::PathInvalid,
            "refusing to overwrite an existing setup target",
        );
    }
    PlatformError::new(ErrorCode::PathInvalid, fallback)
}

fn prompt_line(
    input: &mut dyn std::io::BufRead,
    prompt_out: &mut dyn Write,
    label: &str,
    default: &str,
) -> Result<String, PlatformError> {
    writeln_prompt(prompt_out, label)?;
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "failed to read setup prompt"))?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_owned())
    } else {
        Ok(trimmed.to_owned())
    }
}

fn prompt_bool(
    input: &mut dyn std::io::BufRead,
    prompt_out: &mut dyn Write,
    label: &str,
    default: bool,
) -> Result<bool, PlatformError> {
    let answer = prompt_line(input, prompt_out, label, if default { "y" } else { "n" })?;
    match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "expected y/n for a setup yes/no prompt",
        )),
    }
}

fn writeln_prompt(prompt_out: &mut dyn Write, message: &str) -> Result<(), PlatformError> {
    writeln!(prompt_out, "{message}").map_err(|_| io_failed())
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "failed to write setup output")
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
