//! Scope-level service, status, and instance-list commands.

use super::*;

pub(super) fn run_scope_command(
    cli: &Cli,
    deps: Option<&OperatorDeps>,
    out: &mut impl Write,
) -> Result<bool, PlatformError> {
    let scope = if cli.system {
        ServiceScope::System
    } else {
        ServiceScope::User
    };
    match &cli.command {
        Command::Start | Command::Stop | Command::Restart | Command::Logs { .. } => {
            if cli.config.is_some() || cli.instance.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "daemon service commands select only the user or explicit system OCD scope",
                ));
            }
            let deps = require_operator_deps(deps)?;
            let manager = deps.manager.as_ref();
            match &cli.command {
                Command::Start => {
                    if !manager.is_active(scope)? {
                        manager.start(scope)?;
                    }
                    crate::instance_ops::wait_scoped_daemon_state(
                        &deps.registry,
                        manager,
                        scope,
                        true,
                    )?;
                    writeln!(out, "OCD_STARTED {}", scope.as_str()).map_err(|_| io_failed())?;
                }
                Command::Stop => {
                    if manager.is_active(scope)? {
                        manager.stop(scope)?;
                    }
                    crate::instance_ops::wait_scoped_daemon_state(
                        &deps.registry,
                        manager,
                        scope,
                        false,
                    )?;
                    writeln!(out, "OCD_STOPPED {}", scope.as_str()).map_err(|_| io_failed())?;
                }
                Command::Restart => {
                    manager.restart(scope)?;
                    crate::instance_ops::wait_scoped_daemon_state(
                        &deps.registry,
                        manager,
                        scope,
                        true,
                    )?;
                    writeln!(out, "OCD_RESTARTED {}", scope.as_str()).map_err(|_| io_failed())?;
                }
                Command::Logs { follow } => {
                    write!(out, "{}", manager.logs(scope, *follow)?).map_err(|_| io_failed())?;
                }
                _ => unreachable!("matched daemon service command"),
            }
            Ok(true)
        }
        Command::Status { json } => {
            if cli.config.is_some() || cli.instance.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "`ocd status` selects only the user or explicit system OCD scope",
                ));
            }
            let deps = require_operator_deps(deps)?;
            crate::instance_ops::write_daemon_status(&deps.registry, scope, out, *json)?;
            Ok(true)
        }
        Command::Instances { json } => {
            if cli.config.is_some() || cli.instance.is_some() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "`ocd instances` selects only the user or explicit system OCD scope",
                ));
            }
            let deps = require_operator_deps(deps)?;
            crate::instance_ops::write_instances(&deps.registry, scope, out, *json)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
