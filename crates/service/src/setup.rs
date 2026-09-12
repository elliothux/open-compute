//! Interactive and `--yes` first-host setup for `ocd`.

use crate::config_discover::default_user_config_path;
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
const PROJECT_DATA_REL: &str = ".data/open-compute";

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
    /// Production roots for system, default-user, or project-local setup.
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
            let data_dir = lexical_absolute(startup_cwd, Path::new(PROJECT_DATA_REL))?;
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
        } else if system {
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
        } else {
            let config_path = default_user_config_path()?;
            let data_dir = default_user_data_dir()?;
            let config_parent = config_path
                .parent()
                .ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::ConfigPathInvalid,
                        "default user config path must have a parent directory",
                    )
                })?
                .to_owned();
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
        }
    }
}

fn default_user_data_dir() -> Result<PathBuf, PlatformError> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "HOME is unavailable for user setup",
        )
    })?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "HOME must be absolute for user setup",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        Ok(home.join("Library/Application Support/open-compute/data"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
            && !xdg.is_empty()
        {
            let xdg = PathBuf::from(xdg);
            if !xdg.is_absolute() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "XDG_DATA_HOME must be absolute",
                ));
            }
            return Ok(xdg.join("open-compute"));
        }
        Ok(home.join(".local/share/open-compute"))
    }
}

/// Options for [`run_setup`].
#[derive(Clone, Debug)]
pub struct SetupOptions {
    /// Optional exact configuration path to create.
    pub config: Option<PathBuf>,
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

    let scope = options.scope;

    let (default_config, default_data, default_secrets) = match scope {
        ServiceScope::System => (
            PathBuf::from(SYSTEM_CONFIG_PARENT).join("config.toml"),
            PathBuf::from(SYSTEM_DATA_DIR),
            PathBuf::from(SYSTEM_DATA_DIR).join("secrets"),
        ),
        ServiceScope::User => {
            let (config, data) = if options.config.is_some() {
                (options.config_path.clone(), options.roots.data_dir.clone())
            } else {
                (default_user_config_path()?, default_user_data_dir()?)
            };
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
        service_installed = true;
        manager.install(&record, record.binary_path())?;
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

mod filesystem;

use filesystem::*;

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
