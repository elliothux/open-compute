//! OS service manager adapters for managed ocd instances.

use crate::instance_control::{CONTROL_SCHEMA_VERSION, GenerationDescriptor};
use crate::instance_registry::{InstanceRecord, ServiceScope};
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
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
#[cfg_attr(
    all(not(test), any(target_os = "linux", target_os = "macos")),
    allow(
        dead_code,
        reason = "unsupported-host implementation is compiled only for cross-target coverage"
    )
)]
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

#[cfg_attr(
    all(not(test), any(target_os = "linux", target_os = "macos")),
    allow(
        dead_code,
        reason = "unsupported-host implementation is compiled only for cross-target coverage"
    )
)]
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
        let target = format!("{}/{}", launch_domain(record), record.service_identifier);
        if !launchctl_output(&["print", &target]).is_ok_and(|output| output.status.success()) {
            let path = self.plist_path(record)?;
            launchctl(&["bootstrap", &launch_domain(record), &path.to_string_lossy()])?;
        }
        launchctl(&["kickstart", "-k", &target])
    }

    fn stop(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "bootout",
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

mod fake;

pub use fake::FakeServiceManager;

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
mod tests;
