//! Clap derive CLI for `ocd`.

use crate::backup_cli::{
    backup_attest_restore_smoke, backup_cleanup_incomplete, backup_cleanup_restore, backup_create,
    backup_delete, backup_inspect, backup_list, backup_restore, backup_retention_plan,
    write_result,
};
use crate::capabilities::{platform_capabilities, write_capabilities};
use crate::config_load::{LoadedConfig, load_platform_config, load_platform_config_from};
use crate::doctor::{DoctorMode, doctor_report};
use crate::exit::{ExitClass, emit_failure, exit_class_for};
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::metrics::MetricsRegistry;
use crate::run::run_platform;
use crate::service_manager::{ServiceManager, host_service_manager};
use crate::support_bundle::create_support_bundle;
use crate::target_http::{LiveTargetHttp, TargetHttp};
use crate::target_registry::TargetRegistry;
use clap::{Parser, Subcommand};
use open_compute_core::{
    ErrorCode, GatewayDnsRecordKind, InstanceSelector, PlatformError, PublicGatewayConfig,
};
use open_compute_storage::DataDir;
use std::ffi::OsString;
use std::future::Future;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

mod cache;
mod daemon;
mod instance;
mod loaded;
/// `ocd` command line.
mod model;
mod support;

use loaded::run_loaded;
pub use model::*;
pub use support::load_checked;
#[cfg(test)]
use support::offline_interrupted;
use support::{
    interruptible_offline, io_failed, require_operator_deps, resolve_loaded_config,
    validate_setup_scope, write_config_check, write_gateway_dns_plan,
};

/// Parse argv into [`Cli`].
pub fn parse_from<I, T>(iter: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(iter)
}

/// Injectable registry/manager dependencies for operator commands.
#[derive(Clone)]
pub(crate) struct OperatorDeps {
    /// Instance registry roots.
    pub registry: InstanceRegistry,
    /// Host or fake service manager.
    pub manager: Arc<dyn ServiceManager>,
    /// Scoped remote target registry.
    pub targets: TargetRegistry,
    /// Authenticated target probe transport.
    pub target_http: Arc<dyn TargetHttp>,
}

impl OperatorDeps {
    /// Production registry and host service manager.
    pub(crate) fn production(scope: ServiceScope) -> Result<Self, PlatformError> {
        Ok(Self {
            registry: InstanceRegistry::production()?,
            manager: host_service_manager(),
            targets: TargetRegistry::production(scope)?,
            target_http: Arc::new(LiveTargetHttp::new()?),
        })
    }
}

/// Execute a parsed CLI against stdout/stderr.
pub fn execute<'a>(
    cli: Cli,
    stdout: &'a mut impl Write,
    stderr: &'a mut impl Write,
) -> std::pin::Pin<Box<dyn Future<Output = ExitCode> + 'a>> {
    Box::pin(async move {
        let result = async {
            let startup_cwd = std::env::current_dir().map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "startup working directory is unavailable",
                )
            })?;
            // Avoid requiring HOME/XDG for readonly single-binary commands.
            let deps = if operator_deps_required(&cli) {
                Some(OperatorDeps::production(if cli.system {
                    ServiceScope::System
                } else {
                    ServiceScope::User
                })?)
            } else {
                None
            };
            Box::pin(run(cli, stdout, stderr, &startup_cwd, deps.as_ref())).await
        }
        .await;
        match result {
            Ok(code) => code,
            Err(err) => {
                let _ = emit_failure(&err, stderr);
                ExitCode::from(exit_class_for(err.code()).code())
            }
        }
    })
}

fn operator_deps_required(cli: &Cli) -> bool {
    if matches!(
        &cli.command,
        Command::Caddy {
            command: CaddyCommand::Version
        }
    ) {
        return false;
    }
    if cli.instance.is_some() {
        return true;
    }
    matches!(
        &cli.command,
        Command::Run
            | Command::Instances { .. }
            | Command::Start
            | Command::Stop
            | Command::Restart
            | Command::Status { .. }
            | Command::Logs { .. }
            | Command::Dashboard { .. }
            | Command::Setup { .. }
            | Command::Instance { .. }
            | Command::Cache { .. }
            | Command::Caddy { .. }
            | Command::Upgrade { .. }
            | Command::Uninstall { .. }
            | Command::Purge { .. }
            | Command::Target { .. }
            | Command::Wrangler { .. }
    )
}

/// Test entry that injects registry/manager and a fixed startup cwd.
#[cfg(test)]
pub(crate) async fn execute_with_deps(
    cli: Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    startup_cwd: &Path,
    deps: &OperatorDeps,
) -> ExitCode {
    match run(cli, stdout, stderr, startup_cwd, Some(deps)).await {
        Ok(code) => code,
        Err(err) => {
            let _ = emit_failure(&err, stderr);
            ExitCode::from(exit_class_for(err.code()).code())
        }
    }
}

async fn run(
    cli: Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    startup_cwd: &Path,
    deps: Option<&OperatorDeps>,
) -> Result<ExitCode, PlatformError> {
    let skip_reminder = matches!(
        &cli.command,
        Command::UpdateCheck
            | Command::UpgradePreflight
            | Command::Upgrade { .. }
            | Command::Uninstall { .. }
    );
    let allow_network_refresh =
        !matches!(&cli.command, Command::Run | Command::UpdateCheck) && !skip_reminder;
    let scope = if cli.system {
        ServiceScope::System
    } else {
        ServiceScope::User
    };
    if !skip_reminder
        && let (Ok(cache_path), Ok(exe)) = (
            crate::update_check::default_cache_path(scope),
            std::env::current_exe(),
        )
    {
        let exe = if exe.is_absolute() {
            exe
        } else {
            startup_cwd.join(exe)
        };
        crate::update_check::pre_command_update_check(
            cli.no_update_check,
            allow_network_refresh,
            env!("CARGO_PKG_VERSION"),
            &cache_path,
            &exe,
            scope,
            std::io::stderr().is_terminal(),
            stderr,
        );
    }

    if matches!(&cli.command, Command::UpdateCheck) {
        let cache_path = crate::update_check::default_cache_path(scope)?;
        crate::update_check::run_update_check_helper_live(&cache_path).await?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }

    if matches!(&cli.command, Command::UpgradePreflight) {
        return run_upgrade_preflight(&cli, stdout, startup_cwd);
    }

    if run_project_command(&cli, stdout, stderr, startup_cwd, deps).await? {
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }

    if let Command::Upgrade {
        version,
        dry_run,
        no_restart,
        restore,
    } = &cli.command
    {
        let deps = require_operator_deps(deps)?;
        let options = crate::release_upgrade::UpgradeOptions::production(
            version.clone(),
            *dry_run,
            *no_restart,
            if cli.system {
                ServiceScope::System
            } else {
                ServiceScope::User
            },
        )?;
        if *restore {
            crate::release_upgrade::run_upgrade_restore(
                &options,
                &deps.registry,
                deps.manager.as_ref(),
                stdout,
            )?;
            return Ok(ExitCode::from(ExitClass::Ok.code()));
        }
        let http = crate::release_upgrade::LiveReleaseHttp::new()?;
        crate::release_upgrade::run_upgrade(
            &options,
            &http,
            &deps.registry,
            deps.manager.as_ref(),
            stdout,
        )
        .await?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }

    if let Command::Uninstall {
        purge,
        yes,
        dry_run,
    } = &cli.command
    {
        let deps = require_operator_deps(deps)?;
        let scope = if cli.system {
            ServiceScope::System
        } else {
            ServiceScope::User
        };
        let options = crate::release_upgrade::UpgradeOptions::production(None, true, true, scope)?;
        crate::release_upgrade::run_uninstall(
            &options.receipt_path,
            &options.binary_path,
            &deps.registry,
            deps.manager.as_ref(),
            scope,
            crate::release_upgrade::UninstallOptions {
                purge: *purge,
                yes: *yes,
                dry_run: *dry_run,
            },
            stdout,
        )?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }

    if let Command::Purge { yes, dry_run } = &cli.command {
        let deps = require_operator_deps(deps)?;
        crate::instance_purge::run_selected_purge(
            cli.config.as_deref(),
            cli.instance.as_ref(),
            startup_cwd,
            &deps.registry,
            deps.manager.as_ref(),
            if cli.system {
                ServiceScope::System
            } else {
                ServiceScope::User
            },
            *yes,
            *dry_run,
            stdout,
        )?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }

    if matches!(
        &cli.command,
        Command::Worker {
            command: WorkerCommand::Bundle
        }
    ) {
        crate::worker_cli::encode_bundle(std::io::stdin().lock(), stdout)?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }
    if let Command::Caddy { command } = &cli.command {
        if cli.config.is_some() || cli.instance.is_some() {
            return Err(PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "`ocd caddy` selects only the user or explicit system OCD scope",
            ));
        }
        if matches!(command, CaddyCommand::Version) {
            crate::caddy_cli::write_version(stdout)?;
        } else {
            let deps = require_operator_deps(deps)?;
            crate::caddy_cli::run_offline(&deps.registry, scope, command.clone(), stdout).await?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if daemon::run_scope_command(&cli, deps, stdout)? {
        return Ok(ExitCode::SUCCESS);
    }
    match &cli.command {
        Command::Config {
            command: ConfigCommand::Init { data_dir },
        } => {
            let data_dir = crate::config_load::lexical_absolute(startup_cwd, data_dir)?;
            crate::resources::write_config(&data_dir, stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Licenses => {
            crate::resources::write_licenses(stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Docs { name } => {
            crate::resources::write_docs(name.as_deref(), stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Setup { yes } => {
            if cli.instance.is_some() || cli.config.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "`ocd setup` does not accept --config or --instance; use `ocd instance setup` for a chosen instance path",
                ));
            }
            validate_setup_scope(rustix::process::getuid().is_root(), cli.system)?;
            let deps = require_operator_deps(deps)?;
            let (mut roots, config_path, scope) = crate::setup::SetupRoots::production(cli.system)?;
            roots.system_registry_root = deps.registry.root_for(ServiceScope::System).to_owned();
            roots.user_registry_root = deps.registry.root_for(ServiceScope::User).to_owned();
            let options = crate::setup::SetupOptions {
                yes: *yes,
                roots,
                config_path,
                scope,
            };
            crate::setup::run_setup(&options, startup_cwd, deps.manager.as_ref(), stdout)?;
            return Ok(ExitCode::from(ExitClass::Ok.code()));
        }
        Command::Dashboard { no_open, json } => {
            let deps = require_operator_deps(deps)?;
            crate::instance_ops::open_dashboard(
                cli.config.as_deref(),
                cli.instance.as_ref(),
                startup_cwd,
                &deps.registry,
                if cli.system {
                    ServiceScope::System
                } else {
                    ServiceScope::User
                },
                None,
                *no_open,
                *json,
                stdout,
            )?;
            return Ok(ExitCode::from(ExitClass::Ok.code()));
        }
        Command::Instance { command } => {
            let deps = require_operator_deps(deps)?;
            instance::run(&cli, command, startup_cwd, deps, stdout).await?;
            return Ok(ExitCode::from(ExitClass::Ok.code()));
        }
        Command::Cache { command } => {
            let deps = require_operator_deps(deps)?;
            cache::run(&cli, command, deps, stdout).await?;
            return Ok(ExitCode::from(ExitClass::Ok.code()));
        }
        Command::Capabilities { json } => {
            let loaded = resolve_loaded_config(
                cli.config.as_deref(),
                cli.instance.as_ref(),
                startup_cwd,
                deps.map(|deps| &deps.registry),
                if cli.system {
                    ServiceScope::System
                } else {
                    ServiceScope::User
                },
            )?;
            write_capabilities(&platform_capabilities(&loaded.config)?, stdout, *json)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Run => {
            if cli.config.is_some() || cli.instance.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "`ocd run` selects only the user or explicit system OCD scope",
                ));
            }
            let scope = if cli.system {
                ServiceScope::System
            } else {
                ServiceScope::User
            };
            let deps = require_operator_deps(deps)?;
            Box::pin(run_platform(scope, deps.registry.clone())).await?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }
    let loaded = resolve_loaded_config(
        cli.config.as_deref(),
        cli.instance.as_ref(),
        startup_cwd,
        deps.map(|deps| &deps.registry),
        if cli.system {
            ServiceScope::System
        } else {
            ServiceScope::User
        },
    )?;
    run_loaded(
        cli.command,
        loaded,
        if cli.system {
            ServiceScope::System
        } else {
            ServiceScope::User
        },
        deps.map(|deps| &deps.registry),
        stdout,
    )
    .await
}

fn run_upgrade_preflight(
    cli: &Cli,
    stdout: &mut impl Write,
    startup_cwd: &Path,
) -> Result<ExitCode, PlatformError> {
    let config = cli.config.as_deref().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "upgrade preflight requires an explicit --config path",
        )
    })?;
    if cli.instance.is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "upgrade preflight does not accept --instance",
        ));
    }
    let loaded = load_platform_config_from(config, startup_cwd)?;
    open_compute_storage::PlatformStorage::preflight_upgrade(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )?;
    writeln!(stdout, "UPGRADE_PREFLIGHT_OK")
        .map_err(|_| PlatformError::new(ErrorCode::Internal, "failed to write upgrade output"))?;
    Ok(ExitCode::SUCCESS)
}

async fn run_project_command(
    cli: &Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    startup_cwd: &Path,
    deps: Option<&OperatorDeps>,
) -> Result<bool, PlatformError> {
    if let Command::Target { command } = &cli.command {
        if cli.config.is_some() || cli.instance.is_some() {
            return Err(PlatformError::new(
                ErrorCode::TargetInvalid,
                "ocd target does not accept --config or --instance",
            ));
        }
        let deps = require_operator_deps(deps)?;
        match command {
            TargetCommand::Add {
                name,
                api_base_url,
                instance_id,
                token_file,
            } => crate::target_cli::add_target(
                &deps.targets,
                name.clone(),
                api_base_url.clone(),
                *instance_id,
                token_file.clone(),
                stdout,
            )?,
            TargetCommand::List { json } => {
                crate::target_cli::list_targets(&deps.targets, stdout, *json)?;
            }
            TargetCommand::Show { name, json } => {
                crate::target_cli::show_target(&deps.targets, name, stdout, *json)?;
            }
            TargetCommand::Test { name, json } => {
                crate::target_cli::test_target(
                    &deps.targets,
                    deps.target_http.as_ref(),
                    name,
                    stdout,
                    *json,
                )
                .await?;
            }
            TargetCommand::Remove { name } => {
                crate::target_cli::remove_target(&deps.targets, name, stdout)?;
            }
        }
        return Ok(true);
    }

    if let Command::Wrangler {
        target,
        project,
        arguments,
    } = &cli.command
    {
        let deps = require_operator_deps(deps)?;
        let launch = crate::wrangler_launcher::prepare_wrangler_launch(
            target.as_ref(),
            cli.config.as_deref(),
            cli.instance.as_ref(),
            project.as_deref(),
            arguments,
            startup_cwd,
            &deps.registry,
            if cli.system {
                ServiceScope::System
            } else {
                ServiceScope::User
            },
            &deps.targets,
            deps.target_http.as_ref(),
            None,
            stderr,
        )
        .await?;
        launch.exec(stderr)?;
        unreachable!("successful Wrangler launch replaces the ocd process");
    }

    Ok(false)
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
