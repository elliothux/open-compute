//! One on-demand browser process generation per instance, using P17 process ownership.

use super::{BrowserCdp, BrowserInstallation, BrowserProcess};
use open_compute_core::{BrowserBackendConfig, BrowserConfig, ErrorCode, PlatformError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

// One machine needs a bounded retry gap, not a fleet restart budget or jitter policy.
const RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// A retained process generation; dropping all users permits warm-idle shutdown.
#[derive(Debug)]
pub struct BrowserGeneration {
    id: String,
    contract: [u8; 32],
    cdp: BrowserCdp,
    pub(super) frontend: Arc<super::BrowserFrontend>,
    pub(super) max_message: usize,
    pub(super) queue: usize,
    pub(super) deadline: Duration,
    pub(super) clients: Arc<Semaphore>,
    pub(super) workspace: PathBuf,
    pub(super) max_download_bytes: u64,
    pub(super) max_download_files: u32,
}

impl BrowserGeneration {
    /// Opaque generation identifier used to fence session authority.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Verified executable/resource/version identity for this generation.
    #[must_use]
    pub const fn contract(&self) -> [u8; 32] {
        self.contract
    }

    /// Physical private-pipe connection; only trusted scope owners may use it.
    #[must_use]
    pub const fn cdp(&self) -> &BrowserCdp {
        &self.cdp
    }
}

#[derive(Debug)]
struct Running {
    process: BrowserProcess,
    generation: Arc<BrowserGeneration>,
    workspace: PathBuf,
    idle_since: Option<Instant>,
}

#[derive(Debug, Default)]
struct State {
    running: Option<Running>,
    stopped: bool,
    recovered: bool,
    restart_not_before: Option<Instant>,
}

/// Unique browser process owner beneath an already locked instance data directory.
#[derive(Debug)]
pub struct BrowserManager {
    config: BrowserConfig,
    root: PathBuf,
    state: Mutex<State>,
    pending: Arc<Semaphore>,
}

impl BrowserManager {
    /// Configure lazy startup; no executable is opened and no browser is spawned here.
    pub fn new(config: BrowserConfig, root: PathBuf) -> Result<Arc<Self>, PlatformError> {
        config.validate()?;
        crate::fsutil::require_absolute(&root)?;
        if !matches!(config.backend, BrowserBackendConfig::Managed { .. }) {
            return Err(unavailable());
        }
        let pending = Arc::new(Semaphore::new(config.max_pending_acquires as usize));
        let recovered = Self::recover_orphans(&root)?;
        Ok(Arc::new(Self {
            config,
            root,
            state: Mutex::new(State {
                recovered,
                ..State::default()
            }),
            pending,
        }))
    }

    /// Retain the current healthy generation or start exactly one verified replacement.
    pub async fn acquire(self: &Arc<Self>) -> Result<Arc<BrowserGeneration>, PlatformError> {
        let permit = self.pending.clone().try_acquire_owned().map_err(|_| {
            PlatformError::new(
                ErrorCode::BrowserLimitExceeded,
                "browser startup capacity exceeded",
            )
        })?;
        // Caller cancellation cannot abandon startup before its owned child is installed/reaped.
        let manager = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            manager.acquire_inner().await
        })
        .await
        .map_err(|_| unavailable())?
    }

    async fn acquire_inner(&self) -> Result<Arc<BrowserGeneration>, PlatformError> {
        // Startup and committed shutdown share this lock; a new process cannot overlap reap.
        let mut state = self.state.lock().await;
        if state.stopped {
            return Err(unavailable());
        }
        if state
            .running
            .as_ref()
            .is_some_and(|r| r.process.is_running())
        {
            let running = state.running.as_mut().ok_or_else(unavailable)?;
            running.idle_since = None;
            return Ok(running.generation.clone());
        }
        self.stop_running(&mut state).await?;
        crate::fsutil::create_dir_secure(&self.root)?;
        if !state.recovered {
            Self::recover_orphans(&self.root)?;
            state.recovered = true;
        }
        if let Some(deadline) = state.restart_not_before.take() {
            tokio::time::sleep_until(deadline.into()).await;
        }
        let result = self.start_generation(&mut state).await;
        if result.is_err() {
            state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
        }
        result
    }

    async fn start_generation(
        &self,
        state: &mut State,
    ) -> Result<Arc<BrowserGeneration>, PlatformError> {
        let BrowserBackendConfig::Managed { executable, .. } = &self.config.backend else {
            return Err(unavailable());
        };
        let id = uuid::Uuid::now_v7().to_string();
        let workspace = self.root.join(&id);
        crate::fsutil::create_dir_secure(&workspace)?;
        let installation = BrowserInstallation::open(executable, &workspace).await?;
        let contract = crate::fsutil::parse_sha256_hex(&installation.contract_sha256)?;
        let process = installation
            .spawn_private(
                &workspace,
                self.config.max_message_bytes as usize,
                self.config.max_queued_messages as usize,
                Duration::from_millis(self.config.command_timeout_ms),
            )
            .await?;
        let generation = Arc::new(BrowserGeneration {
            id,
            contract,
            cdp: process.cdp.clone(),
            frontend: installation.frontend.clone(),
            max_message: self.config.max_message_bytes as usize,
            queue: self.config.max_queued_messages as usize,
            deadline: Duration::from_millis(self.config.command_timeout_ms),
            workspace: workspace.clone(),
            max_download_bytes: self.config.max_download_bytes,
            max_download_files: self.config.max_download_files,
            // One authority connection per session plus the explicit frontend connection budget.
            clients: Arc::new(Semaphore::new(
                (self.config.max_sessions + self.config.max_connections) as usize,
            )),
        });
        state.running = Some(Running {
            process,
            generation: generation.clone(),
            workspace,
            idle_since: None,
        });
        Ok(generation)
    }

    /// Reap a crashed generation or a healthy generation after its last user's idle deadline.
    pub async fn reconcile(self: &Arc<Self>) -> Result<(), PlatformError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.reconcile_inner().await })
            .await
            .map_err(|_| unavailable())?
    }

    async fn reconcile_inner(&self) -> Result<(), PlatformError> {
        let mut state = self.state.lock().await;
        let Some(running) = state.running.as_mut() else {
            return Ok(());
        };
        let BrowserBackendConfig::Managed {
            browser_idle_timeout_ms,
            ..
        } = self.config.backend
        else {
            return Err(unavailable());
        };
        let unused = Arc::strong_count(&running.generation) == 1;
        if !unused {
            running.idle_since = None;
        }
        let expired = unused
            && running
                .idle_since
                .get_or_insert_with(Instant::now)
                .elapsed()
                >= Duration::from_millis(browser_idle_timeout_ms);
        if !running.process.is_running() || expired {
            self.stop_running(&mut state).await?;
        }
        Ok(())
    }

    /// Permanently stop admission and complete bounded process-group shutdown.
    pub async fn shutdown(self: &Arc<Self>) -> Result<(), PlatformError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.shutdown_inner().await })
            .await
            .map_err(|_| unavailable())?
    }

    async fn shutdown_inner(&self) -> Result<(), PlatformError> {
        let mut state = self.state.lock().await;
        state.stopped = true;
        self.stop_running(&mut state).await
    }

    async fn stop_running(&self, state: &mut State) -> Result<(), PlatformError> {
        if let Some(running) = state.running.take() {
            let crashed = !running.process.is_running();
            let BrowserBackendConfig::Managed {
                shutdown_grace_ms, ..
            } = self.config.backend
            else {
                return Err(unavailable());
            };
            running
                .process
                .shutdown(Duration::from_millis(shutdown_grace_ms))
                .await;
            crate::fsutil::open_dir_nofollow(&running.workspace)?;
            std::fs::remove_dir_all(&running.workspace).map_err(|_| unavailable())?;
            if crashed {
                state.restart_not_before = Some(Instant::now() + RESTART_BACKOFF);
            }
        }
        Ok(())
    }

    /// Recover protected browser leases without starting an engine, including removed backends.
    /// The caller must hold the instance data-directory lock. Return whether the root existed.
    pub fn recover_orphans(root: &Path) -> Result<bool, PlatformError> {
        crate::fsutil::require_absolute(root)?;
        match std::fs::symlink_metadata(root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(unavailable()),
        }
        crate::fsutil::open_dir_nofollow(root)?;
        let entries = std::fs::read_dir(root).map_err(|_| unavailable())?;
        for (index, entry) in entries.enumerate() {
            if index >= 4096 {
                return Err(unavailable());
            }
            let entry = entry.map_err(|_| unavailable())?;
            let name = entry.file_name();
            if name
                .to_str()
                .is_none_or(|name| uuid::Uuid::parse_str(name).is_err())
            {
                return Err(unavailable());
            }
            let path = entry.path();
            crate::fsutil::open_dir_nofollow(&path)?;
            // The protected lease validates process start identity, group and executable digest.
            let lease = path.join("browser.lease");
            crate::lease::recover_recorded_orphan(&lease)?;
            // Failed-start directories without a lease are retained for diagnosis.
        }
        Ok(true)
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "browser generation unavailable",
    )
}

#[cfg(test)]
#[path = "manager_tests.rs"]
pub(super) mod tests;
