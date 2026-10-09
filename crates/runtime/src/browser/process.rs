//! Private-pipe browser process lifecycle built on the host-process owner.

use super::{BrowserCdp, BrowserInstallation};
use crate::{PersistentHostProcess, PersistentHostProcessSpec};
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;
use tokio::net::UnixStream;

/// One browser generation; the caller owns admission, isolation policy, and its directory.
#[derive(Debug)]
pub struct BrowserProcess {
    process: PersistentHostProcess,
    /// Private bounded CDP channel, never returned to tenants.
    pub cdp: BrowserCdp,
}

impl BrowserInstallation {
    /// Spawn with fixed flags, sandbox enabled, and private CDP fds; await real readiness.
    /// The service owns filesystem/CDP admission; IP filtering belongs to the operator.
    pub async fn spawn_private(
        &self,
        workspace: &Path,
        max_message: usize,
        queue: usize,
        deadline: Duration,
    ) -> Result<BrowserProcess, PlatformError> {
        crate::fsutil::create_dir_secure(&workspace.join("tmp"))?;
        let (parent_input, child_input) =
            std::os::unix::net::UnixStream::pair().map_err(|_| unavailable())?;
        let (child_output, parent_output) =
            std::os::unix::net::UnixStream::pair().map_err(|_| unavailable())?;
        parent_input
            .set_nonblocking(true)
            .map_err(|_| unavailable())?;
        parent_output
            .set_nonblocking(true)
            .map_err(|_| unavailable())?;
        let cdp = BrowserCdp::pipe(
            UnixStream::from_std(parent_input).map_err(|_| unavailable())?,
            UnixStream::from_std(parent_output).map_err(|_| unavailable())?,
            max_message,
            queue,
            deadline,
        )?;
        let mut args: Vec<OsString> = [
            "--remote-debugging-pipe",
            "--site-per-process",
            "--disable-background-networking",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-extensions",
            "--disable-sync",
            "--disable-component-update",
            "--disable-breakpad",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let mut profile = OsString::from("--user-data-dir=");
        profile.push(workspace.join("profile"));
        args.extend([profile, OsString::from("about:blank")]);
        let process = PersistentHostProcess::spawn(
            &self.image,
            PersistentHostProcessSpec {
                args,
                environment: vec![
                    ("HOME".into(), workspace.as_os_str().to_owned()),
                    ("TMPDIR".into(), workspace.join("tmp").into_os_string()),
                ],
                working_directory: workspace.to_owned(),
                private_fds: vec![(child_input.into(), 3), (child_output.into(), 4)],
                lease_path: workspace.join("browser.lease"),
                binary_sha256: self.binary_sha256.clone(),
                redactor: Redactor::new(),
            },
        )?;
        let ready = tokio::time::timeout(deadline, async {
            let version = cdp.command("Browser.getVersion", json!({}), None).await?;
            if !self.frontend.matches_version(&version) {
                return Err(unavailable());
            }
            let targets = cdp.command("Target.getTargets", json!({}), None).await?;
            let targets = targets
                .pointer("/result/targetInfos")
                .and_then(Value::as_array)
                .ok_or_else(unavailable)?;
            for target in targets {
                let target = target
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(unavailable)?;
                let closed = cdp
                    .command("Target.closeTarget", json!({"targetId": target}), None)
                    .await?;
                if closed.pointer("/result/success") != Some(&Value::Bool(true)) {
                    return Err(unavailable());
                }
            }
            // CDP acknowledges close before the target has actually disappeared.
            loop {
                let targets = cdp.command("Target.getTargets", json!({}), None).await?;
                let targets = targets
                    .pointer("/result/targetInfos")
                    .and_then(Value::as_array)
                    .ok_or_else(unavailable)?;
                if targets.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            Ok(())
        })
        .await;
        if !matches!(ready, Ok(Ok(()))) {
            process
                .shutdown(Duration::from_millis(100), Duration::from_secs(1))
                .await;
            return Err(unavailable());
        }
        Ok(BrowserProcess { process, cdp })
    }
}

impl BrowserProcess {
    /// Whether both the process leader and the private CDP owner are alive.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.process.is_running() && self.cdp.is_alive()
    }

    /// Close the physical browser, then perform bounded group termination and complete reap.
    pub async fn shutdown(self, grace: Duration) {
        let _ =
            tokio::time::timeout(grace, self.cdp.command("Browser.close", json!({}), None)).await;
        self.process.shutdown(grace, Duration::from_secs(1)).await;
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeUnavailable, "browser process unavailable")
}
