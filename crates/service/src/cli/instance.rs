//! Scoped instance lifecycle CLI surface.

use super::*;
use crate::instance_ops::ScopedInstanceAction;

pub(super) async fn run(
    cli: &Cli,
    command: &InstanceCommand,
    startup_cwd: &Path,
    deps: &OperatorDeps,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    if cli.instance.is_some()
        || (cli.config.is_some()
            && !matches!(
                command,
                InstanceCommand::Add | InstanceCommand::Setup { .. }
            ))
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance command selection conflicts with global --config or --instance",
        ));
    }
    let scope = if cli.system {
        ServiceScope::System
    } else {
        ServiceScope::User
    };
    match command {
        InstanceCommand::Setup {
            name,
            data_dir,
            yes,
            autostart,
            start,
        } => {
            crate::instance_ops::setup_instance(
                &deps.registry,
                scope,
                cli.config.as_deref(),
                name.as_ref(),
                data_dir.as_deref(),
                *yes,
                *autostart,
                *start,
                startup_cwd,
                out,
            )
            .await
        }
        InstanceCommand::Start { selector } => {
            crate::instance_ops::manage_registered_instance(
                &deps.registry,
                scope,
                selector,
                ScopedInstanceAction::Start,
                out,
            )
            .await
        }
        InstanceCommand::Stop { selector } => {
            crate::instance_ops::manage_registered_instance(
                &deps.registry,
                scope,
                selector,
                ScopedInstanceAction::Stop,
                out,
            )
            .await
        }
        InstanceCommand::Restart { selector } => {
            crate::instance_ops::manage_registered_instance(
                &deps.registry,
                scope,
                selector,
                ScopedInstanceAction::Restart,
                out,
            )
            .await
        }
        InstanceCommand::Add => {
            let config_path = cli.config.as_deref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "instance add requires --config",
                )
            })?;
            crate::instance_ops::add_registered_instance(
                &deps.registry,
                scope,
                config_path,
                startup_cwd,
                out,
            )
            .await
        }
        InstanceCommand::Remove { selector } => {
            crate::instance_ops::remove_registered_instance(&deps.registry, scope, selector, out)
                .await
        }
    }
}
