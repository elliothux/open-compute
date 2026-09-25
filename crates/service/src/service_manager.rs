//! OS service manager adapters for one daemon per selected scope.

use crate::instance_registry::ServiceScope;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Validated non-root user selected by a privileged system setup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemServiceUser {
    /// Login name written into the OS service definition.
    pub name: String,
    /// Numeric user ID used to assign setup-owned files.
    pub uid: u32,
    /// Numeric primary group ID used to assign setup-owned files.
    pub gid: u32,
}

/// Resolve and validate the original non-root caller of `sudo ocd ...`.
pub fn system_service_user() -> Result<SystemServiceUser, PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if std::env::var_os("SUDO_USER").is_none() {
        let name = std::env::var("USER").unwrap_or_else(|_| "test-user".to_owned());
        return Ok(SystemServiceUser {
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
            "system setup requires `sudo` from the non-root user that will run ocd",
        ));
    }
    let actual_uid = lookup_user_id(&name, "-u")?;
    let actual_gid = lookup_user_id(&name, "-g")?;
    if actual_uid != uid || actual_gid != gid {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "sudo service-user identity does not match the local user database",
        ));
    }
    Ok(SystemServiceUser { name, uid, gid })
}

/// Operations required to install and drive one scoped daemon service.
pub trait ServiceManager: Send + Sync {
    /// Install the scoped service definition, accepting only an identical existing file.
    fn install(
        &self,
        scope: ServiceScope,
        service_user: Option<&str>,
        ocd_path: &Path,
    ) -> Result<(), PlatformError>;
    /// Enable the service to start on boot/login.
    fn enable(&self, scope: ServiceScope) -> Result<(), PlatformError>;
    /// Start the service.
    fn start(&self, scope: ServiceScope) -> Result<(), PlatformError>;
    /// Stop the service without disabling it.
    fn stop(&self, scope: ServiceScope) -> Result<(), PlatformError>;
    /// Restart the service and wait for the manager acknowledgement.
    fn restart(&self, scope: ServiceScope) -> Result<(), PlatformError>;
    /// Remove the service definition.
    fn uninstall(&self, scope: ServiceScope) -> Result<(), PlatformError>;
    /// Best-effort manager-reported active state.
    fn is_active(&self, scope: ServiceScope) -> Result<bool, PlatformError>;
    /// Render recent logs for the service.
    fn logs(&self, scope: ServiceScope, follow: bool) -> Result<String, PlatformError>;
    /// Whether this test-only adapter does not launch an actual daemon.
    #[cfg(any(test, feature = "test-support"))]
    fn is_test_stub(&self) -> bool {
        false
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
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct UnsupportedManager;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl ServiceManager for UnsupportedManager {
    fn install(&self, _: ServiceScope, _: Option<&str>, _: &Path) -> Result<(), PlatformError> {
        unsupported()
    }
    fn enable(&self, _: ServiceScope) -> Result<(), PlatformError> {
        unsupported()
    }
    fn start(&self, _: ServiceScope) -> Result<(), PlatformError> {
        unsupported()
    }
    fn stop(&self, _: ServiceScope) -> Result<(), PlatformError> {
        unsupported()
    }
    fn restart(&self, _: ServiceScope) -> Result<(), PlatformError> {
        unsupported()
    }
    fn uninstall(&self, _: ServiceScope) -> Result<(), PlatformError> {
        unsupported()
    }
    fn is_active(&self, _: ServiceScope) -> Result<bool, PlatformError> {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "managed services are only supported on Linux systemd and macOS launchd",
        ))
    }
    fn logs(&self, _: ServiceScope, _: bool) -> Result<String, PlatformError> {
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "managed services are only supported on Linux systemd and macOS launchd",
        ))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unsupported() -> Result<(), PlatformError> {
    Err(PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "managed services are only supported on Linux systemd and macOS launchd",
    ))
}

const SERVICE_LABEL: &str = "dev.open-compute.ocd";

/// Render the one systemd unit for a scope.
pub fn render_systemd_unit(
    scope: ServiceScope,
    service_user: Option<&str>,
    ocd_path: &Path,
) -> Result<String, PlatformError> {
    let (user, system) = match scope {
        ServiceScope::System => (
            format!("User={}\n", required_service_user(service_user)?),
            "--system ",
        ),
        ServiceScope::User => (String::new(), ""),
    };
    let wanted_by = match scope {
        ServiceScope::System => "multi-user.target",
        ServiceScope::User => "default.target",
    };
    Ok(format!(
        "[Unit]\nDescription=Open Compute daemon\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n{user}ExecStart={ocd} {system}run\nRestart=on-failure\nRestartSec=2\nKillMode=control-group\nKillSignal=SIGTERM\nTimeoutStopSec=30\nNoNewPrivileges=yes\n\n[Install]\nWantedBy={wanted_by}\n",
        ocd = shell_escape(&ocd_path.to_string_lossy()),
    ))
}

/// Render the one launchd plist for a scope.
pub fn render_launchd_plist(
    scope: ServiceScope,
    service_user: Option<&str>,
    ocd_path: &Path,
) -> Result<String, PlatformError> {
    let user = match scope {
        ServiceScope::System => format!(
            "  <key>UserName</key>\n  <string>{}</string>\n",
            xml_escape(required_service_user(service_user)?)
        ),
        ServiceScope::User => String::new(),
    };
    let system_arg = if matches!(scope, ServiceScope::System) {
        "    <string>--system</string>\n"
    } else {
        ""
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
{system_arg}
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
        label = SERVICE_LABEL,
        ocd = xml_escape(&ocd_path.to_string_lossy()),
    ))
}

/// systemd user/system adapter.
#[derive(Debug, Default)]
pub struct SystemdManager {
    /// Optional unit root for tests.
    pub unit_root: Option<PathBuf>,
}

impl ServiceManager for SystemdManager {
    fn install(
        &self,
        scope: ServiceScope,
        service_user: Option<&str>,
        ocd_path: &Path,
    ) -> Result<(), PlatformError> {
        let path = self.unit_path(scope)?;
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
        let body = render_systemd_unit(scope, service_user, ocd_path)?;
        install_definition(&path, body.as_bytes(), "systemd unit")?;
        if self.unit_root.is_none() {
            systemctl(scope, &["daemon-reload"])?;
        }
        Ok(())
    }

    fn enable(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(scope, &["enable", unit_name()])
    }

    fn start(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(scope, &["start", unit_name()])
    }

    fn stop(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(scope, &["stop", unit_name()])
    }

    fn restart(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.unit_root.is_some() {
            return Ok(());
        }
        systemctl(scope, &["restart", unit_name()])
    }

    fn uninstall(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        let path = self.unit_path(scope)?;
        if !path.exists() {
            return Ok(());
        }
        if self.unit_root.is_none() {
            systemctl(scope, &["disable", "--now", unit_name()])?;
        }
        fs::remove_file(&path).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to remove systemd unit",
            )
        })?;
        if self.unit_root.is_none() {
            systemctl(scope, &["daemon-reload"])?;
        }
        Ok(())
    }

    fn is_active(&self, scope: ServiceScope) -> Result<bool, PlatformError> {
        if self.unit_root.is_some() {
            return Ok(false);
        }
        let output = systemctl_output(scope, &["is-active", unit_name()])?;
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

    fn logs(&self, scope: ServiceScope, follow: bool) -> Result<String, PlatformError> {
        if follow {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "log follow is not supported in this build path",
            ));
        }
        let unit = unit_name();
        let mut command = Command::new("journalctl");
        if matches!(scope, ServiceScope::User) {
            command.arg("--user");
        }
        let output = command
            .args(["--no-pager", "-u", unit, "-n", "100"])
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
    fn unit_path(&self, scope: ServiceScope) -> Result<PathBuf, PlatformError> {
        let name = unit_name();
        if let Some(root) = &self.unit_root {
            return Ok(root.join(name));
        }
        Ok(match scope {
            ServiceScope::System => PathBuf::from("/etc/systemd/system").join(name),
            ServiceScope::User => crate::instance_registry::user_home_for_uid()?
                .join(".config/systemd/user")
                .join(name),
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
    fn install(
        &self,
        scope: ServiceScope,
        service_user: Option<&str>,
        ocd_path: &Path,
    ) -> Result<(), PlatformError> {
        let path = self.plist_path(scope)?;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
            let _ = ensure_dir_secure(parent);
        }
        let body = render_launchd_plist(scope, service_user, ocd_path)?;
        install_definition(&path, body.as_bytes(), "launchd plist")
    }

    fn enable(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        if launchctl_output(&[
            "print",
            &format!("{}/{}", launch_domain(scope), SERVICE_LABEL),
        ])
        .is_ok_and(|output| output.status.success())
        {
            return Ok(());
        }
        let path = self.plist_path(scope)?;
        launchctl(&["bootstrap", &launch_domain(scope), &path.to_string_lossy()])
    }

    fn start(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        let target = format!("{}/{}", launch_domain(scope), SERVICE_LABEL);
        if !launchctl_output(&["print", &target]).is_ok_and(|output| output.status.success()) {
            let path = self.plist_path(scope)?;
            launchctl(&["bootstrap", &launch_domain(scope), &path.to_string_lossy()])?;
        }
        launchctl(&["kickstart", "-k", &target])
    }

    fn stop(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "bootout",
            &format!("{}/{}", launch_domain(scope), SERVICE_LABEL),
        ])
    }

    fn restart(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.plist_root.is_some() {
            return Ok(());
        }
        launchctl(&[
            "kickstart",
            "-k",
            &format!("{}/{}", launch_domain(scope), SERVICE_LABEL),
        ])
    }

    fn uninstall(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        let path = self.plist_path(scope)?;
        if self.plist_root.is_none() {
            let target = format!("{}/{}", launch_domain(scope), SERVICE_LABEL);
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

    fn is_active(&self, scope: ServiceScope) -> Result<bool, PlatformError> {
        if self.plist_root.is_some() {
            return Ok(false);
        }
        let target = format!("{}/{}", launch_domain(scope), SERVICE_LABEL);
        let output = launchctl_output(&["print", &target])?;
        Ok(output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("state = running"))
    }

    fn logs(&self, _scope: ServiceScope, follow: bool) -> Result<String, PlatformError> {
        let _ = follow;
        Err(PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "launchd log capture is not wired yet; use the plist StandardOutPath",
        ))
    }
}

impl LaunchdManager {
    fn plist_path(&self, scope: ServiceScope) -> Result<PathBuf, PlatformError> {
        let name = format!("{SERVICE_LABEL}.plist");
        if let Some(root) = &self.plist_root {
            return Ok(root.join(name));
        }
        Ok(match scope {
            ServiceScope::System => PathBuf::from("/Library/LaunchDaemons").join(name),
            ServiceScope::User => crate::instance_registry::user_home_for_uid()?
                .join("Library/LaunchAgents")
                .join(name),
        })
    }
}

mod fake;

pub use fake::FakeServiceManager;

fn unit_name() -> &'static str {
    "dev.open-compute.ocd.service"
}

fn launch_domain(scope: ServiceScope) -> String {
    match scope {
        ServiceScope::System => "system".to_owned(),
        ServiceScope::User => format!("gui/{}", rustix::process::getuid().as_raw()),
    }
}

fn systemctl(scope: ServiceScope, args: &[&str]) -> Result<(), PlatformError> {
    let mut command = Command::new("systemctl");
    if matches!(scope, ServiceScope::User) {
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
    scope: ServiceScope,
    args: &[&str],
) -> Result<std::process::Output, PlatformError> {
    let mut command = Command::new("systemctl");
    if matches!(scope, ServiceScope::User) {
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

fn required_service_user(service_user: Option<&str>) -> Result<&str, PlatformError> {
    service_user
        .filter(|user| !user.is_empty() && *user != "root")
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "system service requires a non-root service user",
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

fn lookup_user_id(user: &str, flag: &str) -> Result<u32, PlatformError> {
    let output = Command::new("id")
        .args([flag, user])
        .output()
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to query service user"))?;
    if !output.status.success() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "selected service user does not exist",
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "service user ID is invalid"))
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
