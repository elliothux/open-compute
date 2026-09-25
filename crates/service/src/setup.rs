//! Interactive and `--yes` first-host setup for `ocd`.

use crate::config_load::{lexical_absolute, load_platform_config_from};
use crate::instance_ops::wait_scoped_daemon_state;
use crate::instance_registry::{InstanceRegistry, SYSTEM_REGISTRY_ROOT, ServiceScope};
use crate::service_manager::ServiceManager;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{
    DashboardConfig, ObjectStorageConfig, PlatformConfig, SecretReference,
};
use open_compute_core::{DaemonServerConfig, ErrorCode, PlatformError};
use open_compute_storage::{PlatformStorage, ensure_dir_secure};
use rand::TryRngCore;
use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

const DEFAULT_CONFIG: &str = include_str!("../../../share/default-config.toml");
const TOKEN_BYTES: usize = 32;
const DEFAULT_INSTANCE_NAME: &str = "default";
const SETUP_STAGING_MARKER: &str = "open-compute setup staging\n";

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
    /// Production roots for system or default-user setup.
    pub fn production(system: bool) -> Result<(Self, PathBuf, ServiceScope), PlatformError> {
        let registry = InstanceRegistry::production()?;
        let system_registry_root = registry.root_for(ServiceScope::System).to_owned();
        let user_registry_root = registry.root_for(ServiceScope::User).to_owned();
        if system {
            let config_parent = PathBuf::from(SYSTEM_REGISTRY_ROOT)
                .join("instances")
                .join(DEFAULT_INSTANCE_NAME);
            let data_dir = config_parent.join("data");
            Ok((
                Self {
                    config_parent: config_parent.clone(),
                    secrets_dir: data_dir.join("keys"),
                    data_dir,
                    system_registry_root,
                    user_registry_root,
                },
                config_parent.join("compute.toml"),
                ServiceScope::System,
            ))
        } else {
            let config_parent = user_registry_root
                .join("instances")
                .join(DEFAULT_INSTANCE_NAME);
            let config_path = config_parent.join("compute.toml");
            let data_dir = config_parent.join("data");
            Ok((
                Self {
                    config_parent,
                    secrets_dir: data_dir.join("keys"),
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

/// Options for [`run_setup`].
#[derive(Clone, Debug)]
pub struct SetupOptions {
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

    let default_config = options.config_path.clone();
    let default_data = options.roots.data_dir.clone();

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
    let secrets_dir = data_dir.join("keys");

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
    if plan.secrets_dir != plan.data_dir.join("keys") {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "generated instance secrets must be inside the instance data directory",
        ));
    }
    let mut plan = plan.clone();
    let registry = InstanceRegistry::with_roots(
        plan.system_registry_root.clone(),
        plan.user_registry_root.clone(),
    );
    plan.config_path = crate::instance_registry::normalize_real_path(&plan.config_path)?;
    plan.data_dir = crate::instance_registry::validate_instance_data_path(
        registry.root_for(plan.scope),
        &plan.data_dir,
    )?;
    plan.secrets_dir = plan.data_dir.join("keys");
    let plan = &plan;
    ensure_dir_tree(registry.root_for(plan.scope), plan.scope)?;
    let scope_lock = crate::run::DaemonLock::acquire(registry.root_for(plan.scope))?;
    recover_setup_staging(registry.root_for(plan.scope))?;
    let service_user = match plan.scope {
        ServiceScope::System => Some(crate::service_manager::system_service_user()?),
        ServiceScope::User => None,
    };
    let config_parent = plan.config_path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "setup config path must name a regular file",
        )
    })?;
    let admin_secret = registry.root_for(plan.scope).join("keys/admin.token");
    let deployer_secret = plan.secrets_dir.join("deployer.token");
    let read_only_secret = plan.secrets_dir.join("read-only.token");
    let master_key_file = plan.data_dir.join("keys/master.key");
    let objects_dir = plan.data_dir.join("objects");
    let keys_dir = plan.data_dir.join("keys");

    refuse_nonempty_data(&plan.data_dir)?;
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
    ensure_dir_tree(&registry.root_for(plan.scope).join("instances"), plan.scope)?;
    ensure_dir_tree(&registry.root_for(plan.scope).join("keys"), plan.scope)?;
    let tmp_root = registry.root_for(plan.scope).join("tmp");
    ensure_dir_tree(&tmp_root, plan.scope)?;
    ensure_dir_tree(&plan.data_dir, plan.scope)?;
    ensure_dir_tree(&plan.secrets_dir, plan.scope)?;
    ensure_dir_tree(&keys_dir, plan.scope)?;
    ensure_dir_tree(&objects_dir, plan.scope)?;
    if crate::instance_registry::validate_instance_data_path(
        registry.root_for(plan.scope),
        &plan.data_dir,
    )? != plan.data_dir
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "instance data path changed while creating directories",
        ));
    }

    let staging_name = format!(".ocd-setup-staging-{}", Uuid::now_v7().as_hyphenated());
    let staging_dir = tmp_root.join(&staging_name);
    let mut staging_builder = fs::DirBuilder::new();
    staging_builder.mode(0o700);
    staging_builder
        .create(&staging_dir)
        .map_err(|error| map_privilege(&error, plan.scope, "failed to create setup staging"))?;
    if let Err(err) = prepare_staging(
        plan,
        &staging_dir,
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

    let mut registered = None;
    let mut service_installed = false;
    let mut initialized = false;
    let prepared = (|| {
        if let Some(service_user) = &service_user {
            assign_system_ownership(plan, service_user)?;
        }
        let loaded = load_platform_config_from(&plan.config_path, startup_cwd)?;
        registry.validate_config_data_path(plan.scope, &loaded.path)?;
        if registry
            .list_scope(plan.scope)?
            .iter()
            .any(|record| record.canonical_config_path == loaded.path.to_string_lossy())
        {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "refusing to reuse an existing instance registration during setup",
            ));
        }
        drop(PlatformStorage::bootstrap_with_hardening(
            &loaded.config.data,
            &loaded.config.hardening,
            &SystemClock,
        )?);
        if let Some(service_user) = &service_user {
            assign_initialized_data_ownership(&plan.data_dir, service_user)?;
        }
        initialized = true;
        let record = registry.register_first_with_server(
            &loaded.path,
            plan.scope,
            DaemonServerConfig {
                public_bind: plan.public_bind.clone(),
                admin_bind: None,
                admin_auth: SecretReference {
                    env: None,
                    file: Some(admin_secret.clone()),
                },
            },
            SystemTime::now(),
        )?;
        if let Some(service_user) = &service_user {
            assign_path_ownership(
                &registry.root_for(plan.scope).join("ocd.toml"),
                service_user,
            )?;
        }
        registered = Some(record.clone());
        service_installed = true;
        manager.install(
            plan.scope,
            service_user
                .as_ref()
                .map(|service_user| service_user.name.as_str()),
            &crate::instance_registry::current_binary_path()?,
        )?;
        manager.enable(plan.scope)?;
        Ok(record)
    })();
    let record = match prepared {
        Ok(record) => record,
        Err(error) => {
            if initialized {
                return Err(error);
            }
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

    // The service must acquire the same scope lock before it can start.
    drop(scope_lock);

    // Once activation is attempted, retain the complete registered install on
    // failure. The daemon may have initialized authority in data-dir, so an
    // automatic destructive rollback would be unsafe; `ocd start` can retry it.
    if plan.start_service {
        manager.start(plan.scope)?;
        wait_scoped_daemon_state(&registry, manager, plan.scope, true)?;
    }

    writeln!(
        out,
        "SETUP_OK instance={} config={} scope={} started={}",
        record.instance_id,
        record.canonical_config_path,
        plan.scope.as_str(),
        plan.start_service
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

/// Remove only verified setup staging left by a crashed owner while the caller holds the scope lock.
pub(crate) fn recover_setup_staging(root: &Path) -> Result<(), PlatformError> {
    let tmp = root.join("tmp");
    if fs::symlink_metadata(&tmp).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
        return Ok(());
    }
    if !owned_setup_dir(&tmp) {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "OCD temporary directory has invalid ownership or permissions",
        ));
    }
    let entries = fs::read_dir(&tmp).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to inspect OCD temporary directory",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to inspect OCD temporary entry",
            )
        })?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".ocd-setup-staging-"))
        else {
            continue;
        };
        let recognized = Uuid::parse_str(id).is_ok_and(|uuid| {
            uuid.get_version() == Some(uuid::Version::SortRand) && uuid.to_string() == id
        });
        let path = entry.path();
        if !recognized || !verified_setup_stage(&path) {
            tracing::warn!("skipping unverified OCD setup staging entry");
            continue;
        }
        if fs::remove_dir_all(&path).is_err() {
            tracing::warn!("failed to remove verified OCD setup staging entry");
        }
    }
    Ok(())
}

fn owned_setup_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o700
    })
}

fn verified_setup_stage(path: &Path) -> bool {
    let marker_path = path.join(".owner");
    if !owned_setup_dir(path) || !owned_setup_file(&marker_path) {
        return false;
    }
    let Ok(metadata) = fs::metadata(&marker_path) else {
        return false;
    };
    if metadata.len() > 512 {
        return false;
    }
    let Ok(marker) = fs::read(marker_path) else {
        return false;
    };
    let Some(name) = marker
        .strip_prefix(SETUP_STAGING_MARKER.as_bytes())
        .and_then(|name| name.strip_suffix(b"\n"))
        .and_then(|name| std::str::from_utf8(name).ok())
    else {
        return false;
    };
    if name.is_empty() || Path::new(name).file_name() != Some(std::ffi::OsStr::new(name)) {
        return false;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        match entry.file_name().to_str() {
            Some(".owner") => {}
            Some("secrets") => {
                if !owned_setup_dir(&entry.path()) || !verified_staging_secrets(&entry.path()) {
                    return false;
                }
            }
            Some(found) if found == name => {
                if !owned_setup_file(&entry.path()) {
                    return false;
                }
            }
            Some(_) => return false,
            None => return false,
        }
    }
    true
}

fn verified_staging_secrets(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| {
            matches!(
                entry.file_name().to_str(),
                Some("admin.token" | "deployer.token" | "read-only.token")
            ) && owned_setup_file(&entry.path())
        })
    })
}

fn owned_setup_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o777 == 0o600
    })
}

mod filesystem;
mod instance;

use filesystem::*;
pub(crate) use instance::create_instance;

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
