use super::*;

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
}

impl FakeServiceManager {
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
        if !state.active.contains(&scope) {
            state.active.push(scope);
        }
        Ok(())
    }

    fn stop(&self, scope: ServiceScope) -> Result<(), PlatformError> {
        self.inner
            .lock()
            .map_err(|_| poisoned())?
            .active
            .retain(|value| *value != scope);
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

    #[cfg(any(test, feature = "test-support"))]
    fn is_test_stub(&self) -> bool {
        true
    }
}

fn poisoned() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "fake service manager lock poisoned")
}
