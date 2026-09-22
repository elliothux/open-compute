//! Supervision for the embedded Caddy public gateway.

use open_compute_core::{PlatformError, PublicGatewayConfig, Redactor};
use open_compute_runtime::{PersistentHostProcess, PersistentHostProcessSpec, RuntimePackage};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;
use tokio::sync::watch;

pub(crate) struct GatewayProcess {
    pub(crate) package: RuntimePackage,
    pub(crate) config: PublicGatewayConfig,
    pub(crate) gateway_dir: PathBuf,
    pub(crate) child_pid: Arc<AtomicI32>,
    pub(crate) qualified_pid: Arc<AtomicI32>,
    pub(crate) redactor: Redactor,
    pub(crate) control: Arc<crate::gateway_control::GatewayControl>,
}

impl GatewayProcess {
    pub(crate) async fn run(
        self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let dns_config = self.config.clone();
        let dns_control = self.control.clone();
        let dns_check = tokio::spawn(async move {
            let passed = crate::gateway_dns_verify::verify_public_gateway_dns(&dns_config, &[])
                .await
                .is_ok();
            dns_control.record_dns_result(passed);
        });
        let lease_path = self.gateway_dir.join("run/caddy.lease");
        let mut backoff = Duration::from_secs(1);
        loop {
            let (image, digest) = self.package.caddy()?;
            PersistentHostProcess::recover_orphan(&lease_path, digest)?;
            let snapshot =
                crate::gateway_control::confirmed_snapshot(&self.config, &self.gateway_dir);
            let config_path = snapshot
                .as_ref()
                .cloned()
                .unwrap_or_else(|| self.gateway_dir.join("Caddyfile"));
            let mut args = vec![
                OsString::from("run"),
                OsString::from("--config"),
                config_path.into_os_string(),
            ];
            if snapshot.is_none() {
                args.extend([OsString::from("--adapter"), OsString::from("caddyfile")]);
            }
            let process = PersistentHostProcess::spawn(
                &image,
                PersistentHostProcessSpec {
                    args,
                    environment: vec![
                        (
                            OsString::from("HOME"),
                            self.gateway_dir.clone().into_os_string(),
                        ),
                        (
                            OsString::from("XDG_CONFIG_HOME"),
                            self.gateway_dir.clone().into_os_string(),
                        ),
                        (
                            OsString::from("XDG_DATA_HOME"),
                            self.gateway_dir.clone().into_os_string(),
                        ),
                    ],
                    working_directory: self.gateway_dir.clone(),
                    control_fd: None,
                    lease_path: lease_path.clone(),
                    binary_sha256: digest.to_owned(),
                    redactor: self.redactor.clone(),
                },
            )?;
            let pid = process.pid();
            self.child_pid.store(pid, Ordering::Release);
            let mut probe = tokio::time::interval(Duration::from_secs(5));
            probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let restart = loop {
                tokio::select! {
                    changed = shutdown.changed() => break changed.is_ok() && !*shutdown.borrow(),
                    _ = process.wait_exited() => break true,
                    _ = probe.tick() => {
                        if self.qualified_pid.load(Ordering::Acquire) != pid
                            && process.is_running()
                            && crate::gateway_tls::probe_worker_gateway(
                                self.config.https_listen,
                                &self.config.base_domain,
                                Duration::from_secs(3),
                            ).await.is_ok()
                        {
                            self.qualified_pid.store(pid, Ordering::Release);
                            backoff = Duration::from_secs(1);
                        }
                    }
                }
            };
            self.child_pid.store(0, Ordering::Release);
            self.qualified_pid.store(0, Ordering::Release);
            let _ = process
                .shutdown(Duration::from_secs(5), Duration::from_secs(2))
                .await;
            if !restart {
                dns_check.abort();
                return Ok(());
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown.changed() => return Ok(()),
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}
