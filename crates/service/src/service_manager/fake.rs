use super::*;
use crate::run::daemon_control::{DaemonApi, DaemonSocket};
use std::sync::Mutex;
use tokio::sync::watch;

/// In-memory scoped service manager for unit tests.
#[derive(Clone, Debug, Default)]
pub struct FakeServiceManager {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    installed: Vec<ServiceScope>,
    active: Vec<ServiceScope>,
    fail_install: bool,
    fail_restart: bool,
    // Readiness tests need a live socket, just like a real scoped daemon.
    daemon_root: Option<PathBuf>,
    daemon: Option<(
        watch::Sender<bool>,
        std::thread::JoinHandle<Result<(), PlatformError>>,
    )>,
}

impl FakeServiceManager {
    /// Publish a live daemon control socket when this fake service starts.
    #[must_use]
    pub fn with_daemon(root: &Path) -> Self {
        let manager = Self::default();
        manager.inner.lock().unwrap().daemon_root = Some(root.to_path_buf());
        manager
    }

    /// Force service installation to fail.
    pub fn set_fail_install(&self, fail: bool) {
        if let Ok(mut state) = self.inner.lock() {
            state.fail_install = fail;
        }
    }

    /// Force service restart to fail.
    pub fn set_fail_restart(&self, fail: bool) {
        if let Ok(mut state) = self.inner.lock() {
            state.fail_restart = fail;
        }
    }

    /// Snapshot of installed scopes.
    pub fn installed(&self) -> Vec<ServiceScope> {
        self.inner
            .lock()
            .map(|state| state.installed.clone())
            .unwrap_or_default()
    }

    /// Snapshot of active scopes.
    pub fn started(&self) -> Vec<ServiceScope> {
        self.inner
            .lock()
            .map(|state| state.active.clone())
            .unwrap_or_default()
    }
}

impl ServiceManager for FakeServiceManager {
    fn install(&self, scope: ServiceScope, _: Option<&str>, _: &Path) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| poisoned())?;
        if state.fail_install {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "fake install failure",
            ));
        }
        if !state.installed.contains(&scope) {
            state.installed.push(scope);
        }
        Ok(())
    }

    fn enable(&self, _: ServiceScope) -> Result<(), PlatformError> {
        Ok(())
    }

    fn start(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| poisoned())?;
        if state.daemon.is_none()
            && let Some(root) = &state.daemon_root
        {
            ensure_dir_secure(root)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| poisoned())?;
            let socket = {
                let _guard = runtime.enter();
                DaemonSocket::bind(root)?
            };
            let (api, _commands) = DaemonApi::channel(
                &[],
                vec![],
                open_compute_core::SecretString::new("fake-admin"),
            )?;
            let (stop, receiver) = watch::channel(false);
            let server = std::thread::spawn(move || runtime.block_on(socket.serve(api, receiver)));
            state.daemon = Some((stop, server));
        }
        if !state.active.contains(&scope) {
            state.active.push(scope);
        }
        Ok(())
    }

    fn stop(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        let mut state = self.inner.lock().map_err(|_| poisoned())?;
        if let Some((stop, server)) = state.daemon.take() {
            let _ = stop.send(true);
            server.join().map_err(|_| poisoned())??;
        }
        state.active.retain(|value| *value != scope);
        Ok(())
    }

    fn restart(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        if self.inner.lock().map_err(|_| poisoned())?.fail_restart {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "fake restart failure",
            ));
        }
        self.stop(scope)?;
        self.start(scope)
    }

    fn uninstall(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        self.stop(scope)?;
        let mut state = self.inner.lock().map_err(|_| poisoned())?;
        state.active.retain(|value| *value != scope);
        state.installed.retain(|value| *value != scope);
        Ok(())
    }

    fn is_active(&self, scope: ServiceScope) -> Result<bool, PlatformError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| poisoned())?
            .active
            .contains(&scope))
    }

    fn logs(&self, scope: ServiceScope, _: bool) -> Result<String, PlatformError> {
        Ok(format!("fake logs for {}\n", scope.as_str()))
    }
}

impl Drop for FakeState {
    fn drop(&mut self) {
        if let Some((stop, server)) = self.daemon.take() {
            let _ = stop.send(true);
            let _ = server.join();
        }
    }
}

fn poisoned() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
}
