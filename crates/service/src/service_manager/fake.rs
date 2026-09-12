use super::*;

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
        let mut retained = Vec::new();
        for runtime in state.published_runtimes.drain(..) {
            if runtime.file_name().and_then(|name| name.to_str())
                == Some(record.instance_id.as_str())
            {
                let _ = fs::remove_dir_all(runtime);
            } else {
                retained.push(runtime);
            }
        }
        state.published_runtimes = retained;
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
    let published_at = open_compute_core::wall_time_ms();
    let descriptor = GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: record.instance_id.clone(),
        canonical_config_path: record.canonical_config_path.clone(),
        startup_id: StartupId::generate().to_string(),
        platform_id: PlatformId::generate().to_string(),
        account_id: "0123456789abcdef0123456789abcdef".to_owned(),
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
