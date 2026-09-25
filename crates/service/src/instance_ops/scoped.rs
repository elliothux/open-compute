//! Online instance changes through the daemon-owned control socket.

use super::*;
use crate::run::daemon_control::{ControlRequest, InstanceView, exchange};
use open_compute_storage::InspectLock;
use std::io::{BufRead, IsTerminal};

/// Create a fresh instance through the running daemon after showing exact paths.
#[allow(
    clippy::too_many_arguments,
    reason = "CLI setup options are explicit at the boundary"
)]
pub async fn setup_instance(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    config: Option<&Path>,
    name: Option<&open_compute_core::InstanceName>,
    data_dir: Option<&Path>,
    yes: bool,
    autostart: bool,
    start: bool,
    startup_cwd: &Path,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let root = registry.root_for(scope);
    if read_daemon(root)?.is_none() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "instance setup requires a running scoped daemon; start ocd first",
        ));
    }
    let default_config = root
        .join("instances")
        .join(name.map_or("default", |value| value.as_str()))
        .join("compute.toml");
    let mut config_path =
        crate::config_load::lexical_absolute(startup_cwd, config.unwrap_or(&default_config))?;
    let mut data_path = data_dir
        .map(|path| crate::config_load::lexical_absolute(startup_cwd, path))
        .transpose()?
        .unwrap_or_else(|| config_path.parent().unwrap_or(root).join("data"));
    let mut name = name.cloned();
    let mut autostart = autostart;
    let mut start = start;
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "non-interactive instance setup requires --yes",
            ));
        }
        let mut input = std::io::stdin().lock();
        let mut prompt = std::io::stderr();
        let name_text = prompt_value(
            &mut input,
            &mut prompt,
            "Instance name (blank for none)",
            name.as_ref().map_or("", |value| value.as_str()),
        )?;
        name = if name_text.is_empty() {
            None
        } else {
            Some(name_text.parse()?)
        };
        let config_text = prompt_value(
            &mut input,
            &mut prompt,
            "Configuration path",
            &config_path.to_string_lossy(),
        )?;
        config_path = crate::config_load::lexical_absolute(startup_cwd, Path::new(&config_text))?;
        if data_dir.is_none() {
            data_path = config_path.parent().unwrap_or(root).join("data");
        }
        let data_text = prompt_value(
            &mut input,
            &mut prompt,
            "Data directory",
            &data_path.to_string_lossy(),
        )?;
        data_path = crate::config_load::lexical_absolute(startup_cwd, Path::new(&data_text))?;
        autostart = prompt_yes_no(&mut input, &mut prompt, "Autostart", autostart)?;
        start = prompt_yes_no(&mut input, &mut prompt, "Start now", start)?;
    }
    config_path = crate::instance_registry::normalize_real_path(&config_path)?;
    data_path = crate::instance_registry::validate_instance_data_path(root, &data_path)?;
    writeln!(
        out,
        "INSTANCE_SETUP_PLAN scope={} name={} config={} data_dir={} autostart={} start={}",
        scope.as_str(),
        name.as_ref().map_or("-", |value| value.as_str()),
        config_path.display(),
        data_path.display(),
        autostart,
        start,
    )
    .map_err(|_| io_failed())?;
    if !yes {
        let mut input = std::io::stdin().lock();
        let mut prompt = std::io::stderr();
        if !prompt_yes_no(&mut input, &mut prompt, "Create this instance", false)? {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "instance setup cancelled",
            ));
        }
    }
    send(
        root,
        &ControlRequest::Create {
            config_path: config_path.clone(),
            data_dir: data_path,
            name,
            autostart,
            start,
        },
    )?;
    let record = registry
        .list_scope(scope)?
        .into_iter()
        .find(|record| record.config_path() == config_path)
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::InstanceNotFound, "new instance is unavailable")
        })?;
    let id = record.instance_id()?;
    if start {
        wait_state(root, &id, "running", INSTANCE_READY_TIMEOUT, false).await?;
    }
    writeln!(out, "INSTANCE_CREATED {id}").map_err(|_| io_failed())
}

fn prompt_value(
    input: &mut impl BufRead,
    out: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<String, PlatformError> {
    write!(out, "{label} [{default}]: ").map_err(|_| io_failed())?;
    out.flush().map_err(|_| io_failed())?;
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|_| io_failed())? == 0 {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "instance setup cancelled",
        ));
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_owned()
    } else {
        line.to_owned()
    })
}

fn prompt_yes_no(
    input: &mut impl BufRead,
    out: &mut impl Write,
    label: &str,
    default: bool,
) -> Result<bool, PlatformError> {
    let value = prompt_value(input, out, label, if default { "yes" } else { "no" })?;
    match value.as_str() {
        "yes" | "y" => Ok(true),
        "no" | "n" => Ok(false),
        _ => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "answer yes or no",
        )),
    }
}

pub(crate) fn wait_scoped_daemon_state(
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    scope: ServiceScope,
    running: bool,
) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if manager.is_test_stub() {
        return Ok(());
    }
    let timeout = if running {
        INSTANCE_READY_TIMEOUT
    } else {
        INSTANCE_STOP_TIMEOUT
    };
    let deadline = Instant::now() + timeout;
    loop {
        let manager_running = manager.is_active(scope)?;
        let daemon_running = read_daemon(registry.root_for(scope));
        if manager_running == running
            && daemon_running.is_ok_and(|state| state.is_some() == running)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "scoped daemon did not reach the requested service state",
            ));
        }
        std::thread::sleep(INSTANCE_READY_POLL);
    }
}

pub(crate) fn read_daemon(root: &Path) -> Result<Option<Vec<InstanceView>>, PlatformError> {
    match exchange(root, &ControlRequest::List) {
        Ok(response) if response.ok => Ok(Some(response.instances.unwrap_or_default())),
        Ok(_) => Err(PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "daemon rejected instance listing",
        )),
        Err(_) => {
            let lock = root.join("ocd.lock");
            match std::fs::symlink_metadata(&lock) {
                Ok(_) if InspectLock::try_acquire(&lock)?.is_none() => Err(PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "OCD scope is locked but daemon control is unavailable",
                )),
                Ok(_) => Ok(None),
                Err(io) if io.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "failed to inspect the OCD scope lock",
                )),
            }
        }
    }
}

/// Apply an online instance action without changing its autostart intent.
pub async fn manage_registered_instance(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    selector: &InstanceSelector,
    action: ScopedInstanceAction,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = registry.get_scope(scope, selector)?;
    let id = record.instance_id()?;
    let root = registry.root_for(scope);
    let steps: &[bool] = match action {
        ScopedInstanceAction::Start => &[true],
        ScopedInstanceAction::Stop => &[false],
        ScopedInstanceAction::Restart => &[false, true],
    };
    for start in steps {
        let request = if *start {
            ControlRequest::Start { instance_id: id }
        } else {
            ControlRequest::Stop { instance_id: id }
        };
        send(root, &request)?;
        wait_state(
            root,
            &id,
            if *start { "running" } else { "stopped" },
            if *start {
                INSTANCE_READY_TIMEOUT
            } else {
                INSTANCE_STOP_TIMEOUT
            },
            false,
        )
        .await?;
    }
    let verb = match action {
        ScopedInstanceAction::Start => "STARTED",
        ScopedInstanceAction::Stop => "STOPPED",
        ScopedInstanceAction::Restart => "RESTARTED",
    };
    writeln!(out, "INSTANCE_{verb} {id}").map_err(|_| io_failed())
}

/// Register an initialized compute.toml and start it without rewriting its data path.
pub async fn add_registered_instance(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    config: &Path,
    startup_cwd: &Path,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let loaded = load_platform_config_from(config, startup_cwd)?;
    let root = registry.root_for(scope);
    send(
        root,
        &ControlRequest::Add {
            config_path: loaded.path.clone(),
        },
    )?;
    let record = registry
        .list_scope(scope)?
        .into_iter()
        .find(|record| record.config_path() == loaded.path)
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceNotFound,
                "newly registered instance is unavailable",
            )
        })?;
    let id = record.instance_id()?;
    wait_state(root, &id, "running", INSTANCE_READY_TIMEOUT, false).await?;
    writeln!(out, "INSTANCE_ADDED {id}").map_err(|_| io_failed())
}

/// Stop and remove one registration while preserving its config and data.
pub async fn remove_registered_instance(
    registry: &InstanceRegistry,
    scope: ServiceScope,
    selector: &InstanceSelector,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = registry.get_scope(scope, selector)?;
    let id = record.instance_id()?;
    let root = registry.root_for(scope);
    let state = state(root, &id)?;
    if matches!(state.as_str(), "starting" | "running") {
        send(root, &ControlRequest::Stop { instance_id: id })?;
    }
    if matches!(state.as_str(), "starting" | "running" | "stopping") {
        wait_state(root, &id, "stopped", INSTANCE_STOP_TIMEOUT, true).await?;
    }
    send(root, &ControlRequest::Remove { instance_id: id })?;
    writeln!(out, "INSTANCE_REMOVED {id}").map_err(|_| io_failed())
}

fn send(root: &Path, request: &ControlRequest) -> Result<(), PlatformError> {
    let response = exchange(root, request)?;
    if response.ok {
        return Ok(());
    }
    let code = match response.error.as_deref() {
        Some("INSTANCE_NOT_FOUND") => ErrorCode::InstanceNotFound,
        Some("INSTANCE_REGISTRY_INVALID") => ErrorCode::InstanceRegistryInvalid,
        Some("SECRET_REF_INVALID") => ErrorCode::SecretRefInvalid,
        Some("CONFIG_PATH_INVALID") => ErrorCode::ConfigPathInvalid,
        Some("CONFIG_INVALID") => ErrorCode::ConfigInvalid,
        Some("PATH_INVALID") => ErrorCode::PathInvalid,
        _ => ErrorCode::RuntimeUnavailable,
    };
    Err(PlatformError::new(
        code,
        "daemon rejected instance management request",
    ))
}

fn state(root: &Path, id: &open_compute_core::InstanceId) -> Result<String, PlatformError> {
    let response = exchange(root, &ControlRequest::List)?;
    response
        .instances
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|entry| entry.instance_id == id.as_str())
        .map(|entry| entry.state.clone())
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
        })
}

async fn wait_state(
    root: &Path,
    id: &open_compute_core::InstanceId,
    wanted: &str,
    timeout: Duration,
    failed_is_terminal: bool,
) -> Result<(), PlatformError> {
    let deadline = Instant::now() + timeout;
    loop {
        let current = state(root, id)?;
        if current == wanted || (failed_is_terminal && current == "failed") {
            return Ok(());
        }
        if current == "failed" {
            return Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "instance runtime failed during lifecycle operation",
            ));
        }
        if Instant::now() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "instance lifecycle operation timed out",
            ));
        }
        tokio::time::sleep(INSTANCE_READY_POLL).await;
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn offline_scope_refuses_setup_without_creating_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ocd");
        let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
        let config = root.join("instances/dev/compute.toml");
        let data = root.join("instances/dev/data");
        assert!(read_daemon(&root).unwrap().is_none());
        assert_eq!(
            setup_instance(
                &registry,
                ServiceScope::User,
                Some(&config),
                None,
                Some(&data),
                true,
                false,
                false,
                temp.path(),
                &mut Vec::new(),
            )
            .await
            .unwrap_err()
            .code(),
            ErrorCode::RuntimeUnavailable
        );
        assert!(!root.exists());
        std::fs::create_dir(&root).unwrap();
        let lock_path = root.join("ocd.lock");
        std::fs::write(&lock_path, b"").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let lock = InspectLock::try_acquire(&lock_path).unwrap().unwrap();
        assert_eq!(
            read_daemon(&root).err().unwrap().code(),
            ErrorCode::RuntimeUnavailable
        );
        drop(lock);
        assert!(read_daemon(&root).unwrap().is_none());
    }

    #[test]
    fn eof_cancels_before_using_default() {
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        assert_eq!(
            prompt_value(&mut input, &mut output, "Name", "default")
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid,
        );
    }

    #[test]
    fn prompts_preserve_defaults_and_reject_invalid_or_cancelled_answers() {
        let mut input = std::io::Cursor::new(b"\ncustom\ny\nn\nmaybe\n".as_slice());
        let mut output = Vec::new();
        assert_eq!(
            prompt_value(&mut input, &mut output, "Name", "default").unwrap(),
            "default"
        );
        assert_eq!(
            prompt_value(&mut input, &mut output, "Name", "default").unwrap(),
            "custom"
        );
        assert!(prompt_yes_no(&mut input, &mut output, "Start", false).unwrap());
        assert!(!prompt_yes_no(&mut input, &mut output, "Start", true).unwrap());
        assert_eq!(
            prompt_yes_no(&mut input, &mut output, "Start", true)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid,
        );
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Name [default]: ")
        );
    }
}

#[cfg(test)]
#[path = "scoped_tests.rs"]
mod tests;
