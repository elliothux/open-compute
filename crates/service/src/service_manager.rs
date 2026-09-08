//! OS service manager adapters for managed ocd instances.

use crate::instance_control::{CONTROL_SCHEMA_VERSION, GenerationDescriptor};
use crate::instance_registry::{InstanceRecord, ServiceScope};
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Validated non-root account selected by a privileged system setup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemServiceAccount {
    /// Login name written into the OS service definition.
    pub name: String,
    /// Numeric user ID used to assign setup-owned files.
    pub uid: u32,
    /// Numeric primary group ID used to assign setup-owned files.
    pub gid: u32,
}

/// Resolve and validate the original non-root caller of `sudo ocd ...`.
pub fn system_service_account() -> Result<SystemServiceAccount, PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if std::env::var_os("SUDO_USER").is_none() {
        let name = std::env::var("USER").unwrap_or_else(|_| "test-user".to_owned());
        return Ok(SystemServiceAccount {
            name,
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
        });
    }
    let name = std::env::var("SUDO_USER").unwrap_or_default();
    let uid = parse_sudo_id("SUDO_UID")?;
    let gid = parse_sudo_id("SUDO_GID")?;
    if name.is_empty() || name == "root" || uid == 0 || gid == 0 {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "system setup requires `sudo` from the non-root account that will run ocd",
        ));
    }
    let actual_uid = account_id(&name, "-u")?;
    let actual_gid = account_id(&name, "-g")?;
    if actual_uid != uid || actual_gid != gid {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "sudo service-account identity does not match the local account database",
        ));
    }
    Ok(SystemServiceAccount { name, uid, gid })
}

/// Operations required to install and drive a managed instance unit.
pub trait ServiceManager: Send + Sync {
    /// Install the service definition for `record`, accepting only an identical existing file.
    fn install(&self, record: &InstanceRecord, ocd_path: &Path) -> Result<(), PlatformError>;
    /// Enable the service to start on boot/login.
    fn enable(&self, record: &InstanceRecord) -> Result<(), PlatformError>;
    /// Start the service.
    fn start(&self, record: &InstanceRecord) -> Result<(), PlatformError>;
    /// Stop the service without disabling it.
    fn stop(&self, record: &InstanceRecord) -> Result<(), PlatformError>;
    /// Restart the service and wait for the manager acknowledgement.
    fn restart(&self, record: &InstanceRecord) -> Result<(), PlatformError>;
    /// Remove the service definition.
    fn uninstall(&self, record: &InstanceRecord) -> Result<(), PlatformError>;
    /// Best-effort manager-reported active state.
    fn is_active(&self, record: &InstanceRecord) -> Result<bool, PlatformError>;
    /// Render recent logs for the service.
    fn logs(&self, record: &InstanceRecord, follow: bool) -> Result<String, PlatformError>;
    /// Optional runtime-directory override used when probing readiness after start.
    ///
    /// Production managers return `None` (OS runtime paths). The fake manager
    /// returns its private scratch root so tests never write under `/run`.
    fn readiness_runtime_root(&self) -> Option<PathBuf> {
        None
    }
}

/// Select the host service manager for the current OS.
#[must_use]
pub fn host_service_manager() -> Arc<dyn ServiceManager> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(SystemdManager::default())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(LaunchdManager::default())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Arc::new(UnsupportedManager)
    }
}

/// Placeholder for unsupported hosts.
#[derive(Debug, Default)]
#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
struct UnsupportedManager;

impl ServiceManager for UnsupportedManager {
    fn install(&self, _: &InstanceRecord, _: &Path) -> Result<(), PlatformError> {
        unsupported()
    }
    fn enable(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        unsupported()
    }
    fn start(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        unsupported()
    }
    fn stop(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        unsupported()
    }
    fn restart(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        unsupported()
    }
    fn uninstall(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        unsupported()
    }
    fn is_active(&self, _: &InstanceRecord) -> Result<bool, PlatformError> {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "managed services are only supported on Linux systemd and macOS launchd",
        ))
    }
    fn logs(&self, _: &InstanceRecord, _: bool) -> Result<String, PlatformError> {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "managed services are only supported on Linux systemd and macOS launchd",
        ))
    }
}

#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
fn unsupported() -> Result<(), PlatformError> {
    Err(PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "managed services are only supported on Linux systemd and macOS launchd",
    ))
}

/// Render a systemd unit for one instance.
pub fn render_systemd_unit(
    record: &InstanceRecord,
    ocd_path: &Path,
) -> Result<String, PlatformError> {
    let user = match record.service_scope {
        ServiceScope::System => format!("User={}\n", required_service_user(record)?),
        ServiceScope::User => String::new(),
    };
    let wanted_by = match record.service_scope {
        ServiceScope::System => "multi-user.target",
        ServiceScope::User => "default.target",
    };
    Ok(format!(
        "[Unit]\nDescription=Open Compute daemon ({id})\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n{user}ExecStart={ocd} --config {config} run\nRestart=on-failure\nRestartSec=2\nKillMode=control-group\nKillSignal=SIGTERM\nTimeoutStopSec=30\nNoNewPrivileges=yes\n\n[Install]\nWantedBy={wanted_by}\n",
        id = record.instance_id,
        ocd = shell_escape(&ocd_path.to_string_lossy()),
        config = shell_escape(&record.canonical_config_path),
    ))
}

/// Render a launchd plist for one instance.
pub fn render_launchd_plist(
    record: &InstanceRecord,
    ocd_path: &Path,
) -> Result<String, PlatformError> {
    let user = match record.service_scope {
        ServiceScope::System => format!(
            "  <key>UserName</key>\n  <string>{}</string>\n",
            xml_escape(required_service_user(record)?)
        ),
        ServiceScope::User => String::new(),
    };
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{ocd}</string>
    <string>--config</string>
    <string>{config}</string>
    <string>run</string>
  </array>
{user}  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>2</integer>
  <key>ExitTimeOut</key>
  <integer>30</integer>
</dict>
</plist>
"#,
        label = record.service_identifier,
        ocd = xml_escape(&ocd_path.to_string_lossy()),
        config = xml_escape(&record.canonical_config_path),
    ))
}

/// systemd user/system adapter.
#[derive(Debug, Default)]
pub struct SystemdManager {
    /// Optional unit root for tests.
    pub unit_root: Option<PathBuf>,
}

impl ServiceManager for SystemdManager {
    fn install(&self, record: &InstanceRecord, ocd_path: &Path) -> Result<(), PlatformError> {
        let path = self.unit_path(record)?;
        if let Some(parent) = path.parent() {
            ensure_dir_secure(parent).or_else(|_| {
                fs::create_dir_all(parent).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "failed to create systemd unit directory",
                    )
                })
            })?;
        }
        let body = render_systemd_unit(record, ocd_path)?;
        install_definition(&path, body.as_bytes(), "systemd unit")?;
        if self.unit_root.is_none() {
            systemctl(record, &["daemon-reload"])?;
        }
        Ok(())
    }

    fn enable(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(record, &["enable", &unit_name(record)])
    }

    fn start(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(record, &["start", &unit_name(record)])
    }

    fn stop(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(record, &["stop", &unit_name(record)])
    }

    fn restart(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(record, &["restart", &unit_name(record)])
    }

    fn uninstall(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let path = self.unit_path(record)?;
        if !path.exists() {
            return Ok(());
        }
        if self.unit_root.is_none() {
            systemctl(record, &["disable", "--now", &unit_name(record)])?;
        }
        fs::remove_file(&path).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to remove systemd unit",
            )
        })?;
        if self.unit_root.is_none() {
            systemctl(record, &["daemon-reload"])?;
        }
        Ok(())
    }

    fn is_active(&self, record: &InstanceRecord) -> Result<bool, PlatformError> {
        if self.unit_root.is_some() {
            return Ok(false);
        }
        let output = systemctl_output(record, &["is-active", &unit_name(record)])?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim() == "active");
        }
        if output.status.code() == Some(3) {
            return Ok(false);
        }
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "systemctl could not determine the service state",
        ))
    }

    fn logs(&self, record: &InstanceRecord, follow: bool) -> Result<String, PlatformError> {
        if follow {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "log follow is not supported in this build path",
            ));
        }
        let unit = unit_name(record);
        let output = Command::new("journalctl")
            .args(["--no-pager", "-u", &unit, "-n", "100"])
            .output()
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::PlatformUnavailable,
                    "failed to invoke journalctl",
                )
            })?;
        if !output.status.success() {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "journalctl command failed",
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl SystemdManager {
    fn unit_path(&self, record: &InstanceRecord) -> Result<PathBuf, PlatformError> {
        let name = format!("{}.service", record.service_identifier);
        if let Some(root) = &self.unit_root {
            return Ok(root.join(name));
        }
        Ok(match record.service_scope {
            ServiceScope::System => PathBuf::from("/etc/systemd/system").join(name),
            ServiceScope::User => {
                let home = std::env::var_os("HOME").ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "HOME is unavailable for user systemd units",
                    )
                })?;
                PathBuf::from(home).join(".config/systemd/user").join(name)
            }
        })
    }
}

/// launchd adapter.
#[derive(Debug, Default)]
pub struct LaunchdManager {
    /// Optional plist root for tests.
    pub plist_root: Option<PathBuf>,
}

impl ServiceManager for LaunchdManager {
    fn install(&self, record: &InstanceRecord, ocd_path: &Path) -> Result<(), PlatformError> {
        let path = self.plist_path(record)?;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
            let _ = ensure_dir_secure(parent);
        }
        let body = render_launchd_plist(record, ocd_path)?;
        install_definition(&path, body.as_bytes(), "launchd plist")
    }

    fn enable(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        if launchctl_output(&[
            "print",
            &format!("{}/{}", launch_domain(record), record.service_identifier),
        ])
        .is_ok_and(|output| output.status.success())
        {
            return Ok(());
        }
        let path = self.plist_path(record)?;
        launchctl(&["bootstrap", &launch_domain(record), &path.to_string_lossy()])
    }

    fn start(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "kickstart",
            "-k",
            &format!("{}/{}", launch_domain(record), record.service_identifier),
        ])
    }

    fn stop(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "kill",
            "SIGTERM",
            &format!("{}/{}", launch_domain(record), record.service_identifier),
        ])
    }

    fn restart(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "kickstart",
            "-k",
            &format!("{}/{}", launch_domain(record), record.service_identifier),
        ])
    }

    fn uninstall(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let path = self.plist_path(record)?;
        if self.plist_root.is_none() {
            let target = format!("{}/{}", launch_domain(record), record.service_identifier);
            if let Ok(output) = launchctl_output(&["print", &target])
                && output.status.success()
            {
                launchctl(&["bootout", &target])?;
            }
        }
        if path.exists() {
            fs::remove_file(&path).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to remove launchd plist",
                )
            })?;
        }
        Ok(())
    }

    fn is_active(&self, record: &InstanceRecord) -> Result<bool, PlatformError> {
        if self.plist_root.is_some() {
            return Ok(false);
        }
        let target = format!("{}/{}", launch_domain(record), record.service_identifier);
        let output = launchctl_output(&["print", &target])?;
        Ok(output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("state = running"))
    }

    fn logs(&self, record: &InstanceRecord, follow: bool) -> Result<String, PlatformError> {
        let _ = (record, follow);
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "launchd log capture is not wired yet; use the plist StandardOutPath",
        ))
    }
}

impl LaunchdManager {
    fn plist_path(&self, record: &InstanceRecord) -> Result<PathBuf, PlatformError> {
        let name = format!("{}.plist", record.service_identifier);
        if let Some(root) = &self.plist_root {
            return Ok(root.join(name));
        }
        Ok(match record.service_scope {
            ServiceScope::System => PathBuf::from("/Library/LaunchDaemons").join(name),
            ServiceScope::User => {
                let home = std::env::var_os("HOME").ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "HOME is unavailable for launch agents",
                    )
                })?;
                PathBuf::from(home).join("Library/LaunchAgents").join(name)
            }
        })
    }
}

/// In-memory service manager for tests.
#[derive(Clone, Debug, Default)]
pub struct FakeServiceManager {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    installed: Vec<String>,
    active: Vec<String>,
    fail_install: bool,
    fail_restart: bool,
    /// Private runtime parent used for ready stubs (never `/run` or user XDG).
    ready_runtime_root: Option<PathBuf>,
    published_runtimes: Vec<PathBuf>,
}

impl Drop for FakeState {
    fn drop(&mut self) {
        for path in self.published_runtimes.drain(..) {
            let _ = fs::remove_dir_all(&path);
        }
        if let Some(root) = self.ready_runtime_root.take() {
            let _ = fs::remove_dir_all(&root);
        }
    }
}

impl FakeServiceManager {
    /// Force [`ServiceManager::install`] to fail (setup rollback tests).
    pub fn set_fail_install(&self, fail: bool) {
        if let Ok(mut state) = self.inner.lock() {
            state.fail_install = fail;
        }
    }

    /// Snapshot of installed service identifiers.
    pub fn installed(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|s| s.installed.clone())
            .unwrap_or_default()
    }

    /// Snapshot of service identifiers that have been started.
    pub fn started(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|s| s.active.clone())
            .unwrap_or_default()
    }

    /// Force [`ServiceManager::restart`] to fail (upgrade failure-path tests).
    pub fn set_fail_restart(&self, fail: bool) {
        if let Ok(mut state) = self.inner.lock() {
            state.fail_restart = fail;
        }
    }

    /// Replace the private ready-stub runtime root (tests that probe a fixed path).
    pub fn set_ready_runtime_root(&self, runtime_root: Option<PathBuf>) {
        if let Ok(mut state) = self.inner.lock() {
            state.ready_runtime_root = runtime_root;
        }
    }

    fn ensure_ready_root(state: &mut FakeState) -> Result<PathBuf, PlatformError> {
        if let Some(root) = &state.ready_runtime_root {
            return Ok(root.clone());
        }
        let root =
            std::env::temp_dir().join(format!("oc-fake-rt-{}", Uuid::now_v7().as_hyphenated()));
        fs::create_dir_all(&root).map_err(|_| {
            PlatformError::new(
                ErrorCode::Internal,
                "failed to create fake service manager runtime root",
            )
        })?;
        state.ready_runtime_root = Some(root.clone());
        Ok(root)
    }
}

impl ServiceManager for FakeServiceManager {
    fn install(&self, record: &InstanceRecord, _ocd_path: &Path) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
        })?;
        if state.fail_install {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "fake install failure",
            ));
        }
        if !state.installed.contains(&record.service_identifier) {
            state.installed.push(record.service_identifier.clone());
        }
        Ok(())
    }

    fn enable(&self, _record: &InstanceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn start(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
        })?;
        if !state.active.contains(&record.service_identifier) {
            state.active.push(record.service_identifier.clone());
        }
        let root = Self::ensure_ready_root(&mut state)?;
        let runtime = publish_fake_ready_stub(record, &root)?;
        state.published_runtimes.push(runtime);
        Ok(())
    }

    fn stop(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
        })?;
        state.active.retain(|id| id != &record.service_identifier);
        Ok(())
    }

    fn restart(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        {
            let state = self.inner.lock().map_err(|_| {
                PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
            })?;
            if state.fail_restart {
                return Err(PlatformError::new(
                    ErrorCode::PlatformUnavailable,
                    "fake restart failure",
                ));
            }
        }
        self.stop(record)?;
        self.start(record)
    }

    fn uninstall(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
        })?;
        state.active.retain(|id| id != &record.service_identifier);
        state
            .installed
            .retain(|id| id != &record.service_identifier);
        Ok(())
    }

    fn is_active(&self, record: &InstanceRecord) -> Result<bool, PlatformError> {
        let state = self.inner.lock().map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
        })?;
        Ok(state.active.contains(&record.service_identifier))
    }

    fn logs(&self, record: &InstanceRecord, _follow: bool) -> Result<String, PlatformError> {
        Ok(format!("fake logs for {}\n", record.service_identifier))
    }

    fn readiness_runtime_root(&self) -> Option<PathBuf> {
        self.inner
            .lock()
            .ok()
            .and_then(|state| state.ready_runtime_root.clone())
    }
}

fn publish_fake_ready_stub(
    record: &InstanceRecord,
    runtime_parent: &Path,
) -> Result<PathBuf, PlatformError> {
    let runtime = runtime_parent.join(&record.instance_id);
    fs::create_dir_all(&runtime).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "failed to create fake instance runtime directory",
        )
    })?;
    let published_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            PlatformError::new(ErrorCode::Internal, "system clock is before the Unix epoch")
        })?
        .as_millis();
    let descriptor = GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: record.instance_id.clone(),
        canonical_config_path: record.canonical_config_path.clone(),
        startup_id: StartupId::generate().to_string(),
        platform_id: PlatformId::generate().to_string(),
        release_version: env!("CARGO_PKG_VERSION").to_owned(),
        service_scope: record.service_scope,
        public_listener: None,
        admin_listener: None,
        readiness: "ready".to_owned(),
        published_at: u64::try_from(published_at).unwrap_or(u64::MAX),
    };
    let body = serde_json::to_vec_pretty(&descriptor).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "failed to encode fake ready descriptor",
        )
    })?;
    atomic_write(&runtime.join("descriptor.json"), &body).map_err(|_| {
        PlatformError::new(ErrorCode::Internal, "failed to write fake ready descriptor")
    })?;
    Ok(runtime)
}

fn unit_name(record: &InstanceRecord) -> String {
    format!("{}.service", record.service_identifier)
}

fn launch_domain(record: &InstanceRecord) -> String {
    match record.service_scope {
        ServiceScope::System => "system".to_owned(),
        ServiceScope::User => format!("gui/{}", rustix::process::getuid().as_raw()),
    }
}

fn systemctl(record: &InstanceRecord, args: &[&str]) -> Result<(), PlatformError> {
    let mut command = Command::new("systemctl");
    if matches!(record.service_scope, ServiceScope::User) {
        command.arg("--user");
    }
    let status = command.args(args).status().map_err(|_| {
        PlatformError::new(ErrorCode::PlatformUnavailable, "failed to invoke systemctl")
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "systemctl command failed",
        ))
    }
}

fn systemctl_output(
    record: &InstanceRecord,
    args: &[&str],
) -> Result<std::process::Output, PlatformError> {
    let mut command = Command::new("systemctl");
    if matches!(record.service_scope, ServiceScope::User) {
        command.arg("--user");
    }
    let output = command.args(args).output().map_err(|_| {
        PlatformError::new(ErrorCode::PlatformUnavailable, "failed to invoke systemctl")
    })?;
    Ok(output)
}

fn launchctl(args: &[&str]) -> Result<(), PlatformError> {
    let status = launchctl_output(args)?.status;
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "launchctl command failed",
        ))
    }
}

fn launchctl_output(args: &[&str]) -> Result<std::process::Output, PlatformError> {
    Command::new("launchctl").args(args).output().map_err(|_| {
        PlatformError::new(ErrorCode::PlatformUnavailable, "failed to invoke launchctl")
    })
}

fn required_service_user(record: &InstanceRecord) -> Result<&str, PlatformError> {
    record.service_user.as_deref().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "system service registry entry has no non-root service account",
        )
    })
}

fn parse_sudo_id(name: &str) -> Result<u32, PlatformError> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "system setup requires validated SUDO_USER, SUDO_UID, and SUDO_GID",
            )
        })
}

fn account_id(user: &str, flag: &str) -> Result<u32, PlatformError> {
    let output = Command::new("id")
        .args([flag, user])
        .output()
        .map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to query service account")
        })?;
    if !output.status.success() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "selected service account does not exist",
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "service account ID is invalid"))
}

fn install_definition(path: &Path, body: &[u8], label: &str) -> Result<(), PlatformError> {
    match fs::read(path) {
        Ok(existing) if existing == body => return Ok(()),
        Ok(_) => {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "refusing to replace an existing service definition",
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to inspect an existing service definition",
            ));
        }
    }
    let write_error = match label {
        "systemd unit" => "failed to write systemd unit",
        "launchd plist" => "failed to write launchd plist",
        _ => "failed to write service definition",
    };
    atomic_write(path, body)
        .map_err(|_| PlatformError::new(ErrorCode::InstanceRegistryInvalid, write_error))
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._:-".contains(c))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
#[path = "service_manager_env_tests.rs"]
mod env_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_registry::REGISTRY_SCHEMA_VERSION;

    fn sample_record() -> InstanceRecord {
        InstanceRecord {
            schema_version: REGISTRY_SCHEMA_VERSION,
            instance_id: "k7m2r".to_owned(),
            digest_sha256: "00".repeat(32),
            canonical_config_path: "/etc/open-compute/config.toml".to_owned(),
            service_scope: ServiceScope::User,
            service_user: None,
            service_identifier: "dev.open-compute.ocd.k7m2r".to_owned(),
            created_at: 0,
        }
    }

    #[test]
    fn systemd_unit_embeds_absolute_ocd_and_config() {
        let unit = render_systemd_unit(&sample_record(), Path::new("/usr/local/bin/ocd")).unwrap();
        assert!(
            unit.contains(
                "ExecStart=/usr/local/bin/ocd --config /etc/open-compute/config.toml run"
            )
        );
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("User="));
    }

    #[test]
    fn launchd_plist_embeds_program_arguments() {
        let plist =
            render_launchd_plist(&sample_record(), Path::new("/usr/local/bin/ocd")).unwrap();
        assert!(plist.contains("<string>/usr/local/bin/ocd</string>"));
        assert!(plist.contains("<string>/etc/open-compute/config.toml</string>"));
        assert!(plist.contains("<string>run</string>"));
    }

    #[test]
    fn fake_manager_tracks_lifecycle() {
        let fake = FakeServiceManager::default();
        let record = sample_record();
        fake.install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        fake.start(&record).unwrap();
        assert!(fake.is_active(&record).unwrap());
        fake.stop(&record).unwrap();
        assert!(!fake.is_active(&record).unwrap());
        fake.uninstall(&record).unwrap();
        assert!(fake.installed().is_empty());
    }

    #[test]
    fn render_escapes_shell_and_xml_metacharacters() {
        let mut record = sample_record();
        record.canonical_config_path = "/tmp/weird'path & <x>.toml".to_owned();
        let unit = render_systemd_unit(&record, Path::new("/usr/local/bin/ocd")).unwrap();
        assert!(unit.contains("'/tmp/weird'\\''path & <x>.toml'"));
        let plist = render_launchd_plist(&record, Path::new("/tmp/ocd & <bin>")).unwrap();
        assert!(plist.contains("/tmp/ocd &amp; &lt;bin&gt;"));
        assert!(plist.contains("&amp;") && plist.contains("&lt;"));
    }

    #[test]
    fn launchd_install_and_uninstall_use_plist_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let manager = LaunchdManager {
            plist_root: Some(temp.path().to_path_buf()),
        };
        let record = sample_record();
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        let path = temp
            .path()
            .join(format!("{}.plist", record.service_identifier));
        assert!(path.is_file());
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("dev.open-compute.ocd.k7m2r"));
        manager.enable(&record).unwrap();
        manager.start(&record).unwrap();
        assert!(!manager.is_active(&record).unwrap());
        manager.restart(&record).unwrap();
        assert!(manager.logs(&record, false).is_err());
        manager.uninstall(&record).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn systemd_install_and_uninstall_use_unit_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let manager = SystemdManager {
            unit_root: Some(temp.path().to_path_buf()),
        };
        let record = sample_record();
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        let path = temp
            .path()
            .join(format!("{}.service", record.service_identifier));
        assert!(path.is_file());
        manager.enable(&record).unwrap();
        manager.start(&record).unwrap();
        manager.stop(&record).unwrap();
        manager.restart(&record).unwrap();
        assert!(!manager.is_active(&record).unwrap());
        assert!(manager.logs(&record, true).is_err());
        manager.uninstall(&record).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn unsupported_manager_fails_closed() {
        let manager = UnsupportedManager;
        let record = sample_record();
        assert!(
            manager
                .install(&record, Path::new("/usr/local/bin/ocd"))
                .is_err()
        );
        assert!(manager.enable(&record).is_err());
        assert!(manager.start(&record).is_err());
        assert!(manager.stop(&record).is_err());
        assert!(manager.restart(&record).is_err());
        assert!(manager.uninstall(&record).is_err());
        assert!(manager.is_active(&record).is_err());
        assert!(manager.logs(&record, false).is_err());
    }

    #[test]
    fn host_service_manager_returns_platform_adapter() {
        let manager = host_service_manager();
        // Only exercise the constructor; OS command paths are not invoked here.
        let _ = manager.logs(&sample_record(), true);
    }

    #[test]
    fn systemd_without_unit_root_fails_closed_on_missing_systemctl() {
        let manager = SystemdManager { unit_root: None };
        let mut record = sample_record();
        record.service_scope = ServiceScope::User;
        record.service_identifier = "dev.open-compute.ocd.coverage-miss".to_owned();
        // On hosts without systemd these invoke fail immediately; do not install units.
        assert!(manager.enable(&record).is_err());
        assert!(manager.start(&record).is_err());
        assert!(manager.stop(&record).is_err());
        assert!(manager.restart(&record).is_err());
        let _ = manager.is_active(&record);
        let _ = manager.logs(&record, false);
        assert!(manager.logs(&record, true).is_err());
    }

    #[test]
    fn systemd_install_creates_nested_unit_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let nested = temp.path().join("nested/units");
        let manager = SystemdManager {
            unit_root: Some(nested.clone()),
        };
        let mut record = sample_record();
        record.service_scope = ServiceScope::System;
        record.service_user = Some("ocd-service".to_owned());
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        assert!(
            nested
                .join(format!("{}.service", record.service_identifier))
                .is_file()
        );
        // Missing unit file uninstall is a no-op after remove.
        manager.uninstall(&record).unwrap();
        manager.uninstall(&record).unwrap();
    }

    #[test]
    fn launchd_nested_plist_root_and_missing_uninstall() {
        let temp = tempfile::TempDir::new().unwrap();
        let nested = temp.path().join("agents/nested");
        let manager = LaunchdManager {
            plist_root: Some(nested.clone()),
        };
        let mut record = sample_record();
        record.service_scope = ServiceScope::System;
        record.service_user = Some("ocd-service".to_owned());
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        assert!(
            nested
                .join(format!("{}.plist", record.service_identifier))
                .is_file()
        );
        manager.stop(&record).unwrap();
        manager.uninstall(&record).unwrap();
        manager.uninstall(&record).unwrap();
    }

    #[test]
    fn fake_manager_restart_failure_and_logs() {
        let fake = FakeServiceManager::default();
        let record = sample_record();
        fake.install(&record, Path::new("/usr/local/bin/ocd"))
            .unwrap();
        fake.enable(&record).unwrap();
        fake.start(&record).unwrap();
        assert!(fake.logs(&record, false).unwrap().contains("fake logs"));
        fake.set_fail_restart(true);
        assert_eq!(
            fake.restart(&record).unwrap_err().code(),
            ErrorCode::PlatformUnavailable
        );
        fake.set_fail_restart(false);
        fake.restart(&record).unwrap();
        assert!(fake.is_active(&record).unwrap());
        assert_eq!(fake.started(), vec![record.service_identifier.clone()]);
    }

    #[test]
    fn render_covers_system_scope_identifiers() {
        let mut record = sample_record();
        record.service_scope = ServiceScope::System;
        record.service_user = Some("ocd-service".to_owned());
        let unit = render_systemd_unit(&record, Path::new("/opt/ocd")).unwrap();
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(unit.contains("User=ocd-service"));
        let plist = render_launchd_plist(&record, Path::new("/opt/ocd")).unwrap();
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("<key>UserName</key>"));
        assert_eq!(
            unit_name(&record),
            format!("{}.service", record.service_identifier)
        );
        assert_eq!(launch_domain(&record), "system");
    }

    #[test]
    fn launchd_without_plist_root_fails_closed_on_launchctl() {
        let manager = LaunchdManager { plist_root: None };
        let mut record = sample_record();
        record.service_identifier = "dev.open-compute.ocd.coverage-launchd".to_owned();
        // Real launchctl against a missing unit should fail closed quickly.
        assert!(manager.enable(&record).is_err());
        assert!(manager.start(&record).is_err());
        assert!(manager.stop(&record).is_err());
        let _ = manager.restart(&record);
        let _ = manager.is_active(&record);
        assert!(manager.logs(&record, false).is_err());
        assert!(manager.logs(&record, true).is_err());
    }

    #[test]
    fn launch_domain_covers_user_gui_scope() {
        let record = sample_record();
        assert!(launch_domain(&record).starts_with("gui/"));
    }

    #[test]
    fn systemd_unit_path_and_install_fail_closed() {
        let manager = SystemdManager { unit_root: None };
        let mut record = sample_record();
        record.service_scope = ServiceScope::System;
        let path = manager.unit_path(&record).unwrap();
        assert!(path.starts_with("/etc/systemd/system"));
        record.service_scope = ServiceScope::User;
        let path = manager.unit_path(&record).unwrap();
        assert!(path.to_string_lossy().contains(".config/systemd/user"));

        let temp = tempfile::TempDir::new().unwrap();
        let file_root = temp.path().join("not-a-dir");
        fs::write(&file_root, b"x").unwrap();
        let bad = SystemdManager {
            unit_root: Some(file_root),
        };
        assert!(
            bad.install(&sample_record(), Path::new("/usr/local/bin/ocd"))
                .is_err()
        );

        let ok_root = tempfile::TempDir::new().unwrap();
        let manager = SystemdManager {
            unit_root: Some(ok_root.path().to_path_buf()),
        };
        let record = sample_record();
        // Pre-create the unit path as a directory so atomic_write fails closed.
        let unit = manager.unit_path(&record).unwrap();
        fs::create_dir_all(&unit).unwrap();
        assert!(
            manager
                .install(&record, Path::new("/usr/local/bin/ocd"))
                .is_err()
        );
    }

    #[test]
    fn launchd_install_fails_when_plist_path_is_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let manager = LaunchdManager {
            plist_root: Some(temp.path().to_path_buf()),
        };
        let record = sample_record();
        let path = manager.plist_path(&record).unwrap();
        fs::create_dir_all(&path).unwrap();
        assert!(
            manager
                .install(&record, Path::new("/usr/local/bin/ocd"))
                .is_err()
        );
    }

    #[test]
    fn service_definition_install_is_idempotent_but_never_replaces_content() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("service.unit");
        install_definition(&path, b"first", "custom").unwrap();
        install_definition(&path, b"first", "custom").unwrap();
        let err = install_definition(&path, b"second", "custom").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
        assert_eq!(fs::read(&path).unwrap(), b"first");

        let directory = temp.path().join("directory");
        fs::create_dir(&directory).unwrap();
        let err = install_definition(&directory, b"body", "custom").unwrap_err();
        assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    }

    #[test]
    fn system_service_definitions_require_an_account() {
        let mut record = sample_record();
        record.service_scope = ServiceScope::System;
        let unit_error = render_systemd_unit(&record, Path::new("/opt/ocd")).unwrap_err();
        assert_eq!(unit_error.code(), ErrorCode::InstanceRegistryInvalid);
        let plist_error = render_launchd_plist(&record, Path::new("/opt/ocd")).unwrap_err();
        assert_eq!(plist_error.code(), ErrorCode::InstanceRegistryInvalid);
    }

    #[test]
    fn uninstall_propagates_definition_removal_errors() {
        let systemd_root = tempfile::TempDir::new().unwrap();
        let systemd = SystemdManager {
            unit_root: Some(systemd_root.path().to_path_buf()),
        };
        let record = sample_record();
        fs::create_dir(systemd.unit_path(&record).unwrap()).unwrap();
        assert_eq!(
            systemd.uninstall(&record).unwrap_err().code(),
            ErrorCode::InstanceRegistryInvalid
        );

        let launchd_root = tempfile::TempDir::new().unwrap();
        let launchd = LaunchdManager {
            plist_root: Some(launchd_root.path().to_path_buf()),
        };
        fs::create_dir(launchd.plist_path(&record).unwrap()).unwrap();
        assert_eq!(
            launchd.uninstall(&record).unwrap_err().code(),
            ErrorCode::InstanceRegistryInvalid
        );
    }
}
