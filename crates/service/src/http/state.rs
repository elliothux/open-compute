use super::*;
use open_compute_core::DaemonServerConfig;

/// Shared HTTP transport state composed by the service root.
#[derive(Clone)]
pub struct HttpState {
    pub(super) platform: PlatformState,
    pub(super) listener: ListenerState,
    pub(super) auth: AuthState,
    pub(super) products: ProductState,
    pub(super) dashboard: DashboardState,
    #[cfg(any(test, feature = "test-support"))]
    pub(super) test_runtime_restart: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

#[derive(Clone)]
pub(super) struct PlatformState {
    pub(super) health: HealthCoordinator,
    pub(super) metrics: Arc<MetricsRegistry>,
    pub(super) metrics_enabled: bool,
    pub(super) capability_limits: Arc<BTreeMap<String, u64>>,
}

#[derive(Clone, Default)]
pub(super) struct ListenerState {
    pub(super) local_origin_port: Option<u16>,
    pub(super) control_origin_port: Option<u16>,
    pub(super) public_gateway_process: Option<(Arc<AtomicI32>, Arc<AtomicI32>)>,
}

#[derive(Clone, Default)]
pub(super) struct AuthState {
    pub(super) admin_secret: Option<Arc<SecretString>>,
    pub(super) deployer_secret: Option<Arc<SecretString>>,
    pub(super) read_only_secret: Option<Arc<SecretString>>,
}

#[derive(Clone, Default)]
pub(super) struct ProductState {
    pub(super) v4_instance_context: Option<Arc<V4InstanceContext>>,
    pub(super) platform_storage: Option<Arc<PlatformStorage>>,
    pub(super) worker_api: Option<Arc<WorkerApiState>>,
    pub(super) kv_api: Option<Arc<KvApiState>>,
    pub(super) r2_api: Option<Arc<R2ApiState>>,
    pub(super) d1_api: Option<Arc<D1ApiState>>,
    pub(super) artifact_api: Option<Arc<ArtifactApiState>>,
    pub(super) queue_api: Option<Arc<QueueApiState>>,
    pub(super) workflow_api: Option<Arc<WorkflowApiState>>,
    pub(super) scheduler: Option<Arc<SchedulerService>>,
    pub(super) cache_images_api: Option<Arc<CacheImagesApiState>>,
    pub(super) search_api: Option<Arc<SearchApiState>>,
    pub(super) browser: Option<Arc<crate::browser::BrowserService>>,
}

#[derive(Clone)]
pub(super) struct DashboardState {
    pub(super) enabled: bool,
    pub(super) dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    pub(super) auth: Option<Arc<DashboardAuth>>,
}

impl std::fmt::Debug for HttpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpState")
            .field("metrics_enabled", &self.platform.metrics_enabled)
            .field("dashboard_enabled", &self.dashboard.enabled)
            .field(
                "local_origin_available",
                &self.listener.local_origin_port.is_some(),
            )
            .field("admin_auth", &self.auth.admin_secret.is_some())
            .field("deployer_auth", &self.auth.deployer_secret.is_some())
            .field("read_only_auth", &self.auth.read_only_secret.is_some())
            .field(
                "v4_instance_context",
                &self.products.v4_instance_context.is_some(),
            )
            .field(
                "platform_storage",
                &self.products.platform_storage.is_some(),
            )
            .field("capability_limits", &self.platform.capability_limits.len())
            .field(
                "test_runtime_restart",
                &cfg!(any(test, feature = "test-support")),
            )
            .field("worker_api", &self.products.worker_api.is_some())
            .field("kv_api", &self.products.kv_api.is_some())
            .field("r2_api", &self.products.r2_api.is_some())
            .field("d1_api", &self.products.d1_api.is_some())
            .field("artifact_api", &self.products.artifact_api.is_some())
            .field("queue_api", &self.products.queue_api.is_some())
            .field("workflow_api", &self.products.workflow_api.is_some())
            .field("scheduler", &self.products.scheduler.is_some())
            .field(
                "cache_images_api",
                &self.products.cache_images_api.is_some(),
            )
            .field("dashboard_dispatch", &"<async>")
            .field("search_api", &self.products.search_api.is_some())
            .field("dashboard_auth", &self.dashboard.auth.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpState {
    pub(crate) fn validate_composed(&self) -> Result<(), PlatformError> {
        let products = &self.products;
        let complete = [
            products.v4_instance_context.is_some(),
            products.platform_storage.is_some(),
            products.worker_api.is_some(),
            products.kv_api.is_some(),
            products.r2_api.is_some(),
            products.d1_api.is_some(),
            products.artifact_api.is_some(),
            products.queue_api.is_some(),
            products.workflow_api.is_some(),
            products.scheduler.is_some(),
            products.cache_images_api.is_some(),
            products.search_api.is_some(),
            self.auth.admin_secret.is_some(),
            self.auth.deployer_secret.is_some(),
            self.auth.read_only_secret.is_some(),
            !self.dashboard.enabled || self.dashboard.auth.is_some(),
        ]
        .into_iter()
        .all(|present| present);
        if complete {
            Ok(())
        } else {
            Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "HTTP composition is incomplete",
            ))
        }
    }

    /// Build state. Resolves admin auth when configured.
    pub fn new(
        health: HealthCoordinator,
        metrics: Arc<MetricsRegistry>,
        metrics_enabled: bool,
        dashboard_enabled: bool,
        server: &DaemonServerConfig,
        auth: &InstanceAuthConfig,
    ) -> Result<Self, PlatformError> {
        let admin_secret = Arc::new(resolve_admin_auth(&server.admin_auth)?);
        let deployer_secret = Arc::new(resolve_bearer_auth(&auth.deployer_auth)?);
        let read_only_secret = Arc::new(resolve_bearer_auth(&auth.read_only_auth)?);
        if admin_secret.expose() == deployer_secret.expose()
            || admin_secret.expose() == read_only_secret.expose()
            || deployer_secret.expose() == read_only_secret.expose()
        {
            return Err(PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "instance Bearer tokens must be distinct",
            ));
        }
        Ok(Self {
            platform: PlatformState {
                health,
                metrics,
                metrics_enabled,
                capability_limits: Arc::new(BTreeMap::new()),
            },
            listener: ListenerState::default(),
            auth: AuthState {
                admin_secret: Some(admin_secret),
                deployer_secret: Some(deployer_secret),
                read_only_secret: Some(read_only_secret),
            },
            products: ProductState::default(),
            dashboard: DashboardState {
                enabled: dashboard_enabled,
                dispatch: Arc::new(RwLock::new(None)),
                auth: None,
            },
            #[cfg(any(test, feature = "test-support"))]
            test_runtime_restart: None,
        })
    }

    /// Attach Dashboard one-time login and short browser session authority.
    #[must_use]
    pub fn with_dashboard_auth(mut self, auth: Arc<DashboardAuth>) -> Self {
        self.dashboard.auth = Some(auth);
        self
    }

    /// Borrow Dashboard auth when this process generation enabled it.
    #[must_use]
    pub(crate) fn dashboard_auth(&self) -> Option<&DashboardAuth> {
        self.dashboard.auth.as_deref()
    }

    /// Share the dashboard dispatch slot populated after runtime bootstrap.
    #[must_use]
    pub fn with_dashboard_dispatch(
        mut self,
        dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    ) -> Self {
        self.dashboard.dispatch = dispatch;
        self
    }

    /// Test helper with no admin auth.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        health: HealthCoordinator,
        metrics: Arc<MetricsRegistry>,
        metrics_enabled: bool,
        admin_secret: Option<SecretString>,
    ) -> Self {
        Self {
            platform: PlatformState {
                health,
                metrics,
                metrics_enabled,
                capability_limits: Arc::new(BTreeMap::new()),
            },
            listener: ListenerState::default(),
            auth: AuthState {
                admin_secret: admin_secret.map(Arc::new),
                ..AuthState::default()
            },
            products: ProductState::default(),
            dashboard: DashboardState {
                enabled: false,
                dispatch: Arc::new(RwLock::new(None)),
                auth: None,
            },
            test_runtime_restart: None,
        }
    }

    /// Enable dashboard surface responses in tests without runtime bootstrap.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_dashboard_enabled(mut self, enabled: bool) -> Self {
        self.dashboard.enabled = enabled;
        self
    }

    /// Publish the validated installation-local product limit registry.
    #[must_use]
    pub fn with_capability_limits(mut self, limits: BTreeMap<String, u64>) -> Self {
        self.platform.capability_limits = Arc::new(limits);
        self
    }

    /// Borrow the validated installation-local product limit registry.
    #[must_use]
    pub(crate) fn capability_limits(&self) -> &BTreeMap<String, u64> {
        &self.platform.capability_limits
    }

    /// Attach the P0.2 control/data plane to this listener state.
    #[must_use]
    pub fn with_worker_api(mut self, worker_api: WorkerApiState) -> Self {
        self.products.worker_api = Some(Arc::new(worker_api));
        self
    }

    /// Publish local Worker origins for an actually bound loopback listener.
    #[must_use]
    pub fn with_local_origin_addr(mut self, address: std::net::SocketAddr) -> Self {
        self.listener.local_origin_port = address.ip().is_loopback().then_some(address.port());
        self
    }

    /// Publish URLs only for the actually bound loopback control-plane listener.
    #[must_use]
    pub fn with_control_origin_addr(mut self, address: std::net::SocketAddr) -> Self {
        self.listener.control_origin_port = address.ip().is_loopback().then_some(address.port());
        self
    }

    pub(crate) const fn control_origin_port(&self) -> Option<u16> {
        self.listener.control_origin_port
    }

    /// Return the bound local-origin port when this listener is loopback-reachable.
    #[must_use]
    pub(crate) const fn local_origin_port(&self) -> Option<u16> {
        self.listener.local_origin_port
    }

    /// Bind the trusted child PID and separately qualified PID to public endpoint admission.
    #[must_use]
    pub fn with_public_gateway_process(
        mut self,
        child_pid: Arc<AtomicI32>,
        qualified_pid: Arc<AtomicI32>,
    ) -> Self {
        self.listener.public_gateway_process = Some((child_pid, qualified_pid));
        self
    }

    /// Publish only the currently running child after its TLS qualification succeeds.
    #[must_use]
    pub(crate) fn public_gateway_serving(&self) -> bool {
        self.listener
            .public_gateway_process
            .as_ref()
            .is_some_and(|(child_pid, qualified_pid)| {
                let child = child_pid.load(Ordering::Acquire);
                child > 0 && qualified_pid.load(Ordering::Acquire) == child
            })
    }

    /// Attach a generic supervised-runtime restart hook to test-support builds.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub(crate) fn with_test_runtime_restart(
        mut self,
        restart: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.test_runtime_restart = Some(restart);
        self
    }

    /// Borrow the optional P0.2 API state.
    #[must_use]
    pub(crate) fn worker_api(&self) -> Option<&Arc<WorkerApiState>> {
        self.products.worker_api.as_ref()
    }

    /// Attach the P0.4 KV control plane to this listener state.
    #[must_use]
    pub fn with_kv_api(mut self, kv_api: KvApiState) -> Self {
        self.products.kv_api = Some(Arc::new(kv_api));
        self
    }

    /// Borrow the optional P0.4 API state.
    #[must_use]
    pub(crate) fn kv_api(&self) -> Option<&Arc<KvApiState>> {
        self.products.kv_api.as_ref()
    }

    /// Attach the P0.5 R2 logical-bucket control plane.
    #[must_use]
    pub fn with_r2_api(mut self, r2_api: R2ApiState) -> Self {
        self.products.r2_api = Some(Arc::new(r2_api));
        self
    }

    /// Borrow the optional P0.5 R2 control-plane state.
    #[must_use]
    pub(crate) fn r2_api(&self) -> Option<&Arc<R2ApiState>> {
        self.products.r2_api.as_ref()
    }

    /// Attach the P0.6 D1 control plane.
    #[must_use]
    pub fn with_d1_api(mut self, d1_api: D1ApiState) -> Self {
        self.products.d1_api = Some(Arc::new(d1_api));
        self
    }

    /// Borrow the optional P0.6 D1 control-plane state.
    #[must_use]
    pub(crate) fn d1_api(&self) -> Option<&Arc<D1ApiState>> {
        self.products.d1_api.as_ref()
    }

    /// Attach the Cloudflare Artifacts authority.
    #[must_use]
    pub(crate) fn with_artifact_api(mut self, api: ArtifactApiState) -> Self {
        self.products.artifact_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional Cloudflare Artifacts authority.
    #[must_use]
    pub(crate) fn artifact_api(&self) -> Option<&Arc<ArtifactApiState>> {
        self.products.artifact_api.as_ref()
    }

    /// Attach the P2.2 Queue catalog control plane.
    #[must_use]
    pub fn with_queue_api(mut self, queue_api: Option<QueueApiState>) -> Self {
        self.products.queue_api = queue_api.map(Arc::new);
        self
    }

    /// Borrow the optional P2.2 Queue control-plane state.
    #[must_use]
    pub(crate) fn queue_api(&self) -> Option<&Arc<QueueApiState>> {
        self.products.queue_api.as_ref()
    }

    /// Attach the Workflow catalog and bounded operator history.
    #[must_use]
    pub fn with_workflow_api(mut self, workflow_api: Option<WorkflowApiState>) -> Self {
        self.products.workflow_api = workflow_api.map(Arc::new);
        self
    }

    pub(crate) fn workflow_api(&self) -> Option<&Arc<WorkflowApiState>> {
        self.products.workflow_api.as_ref()
    }

    /// Attach the P0.8 scheduler operator surface.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerService>>) -> Self {
        self.products.scheduler = scheduler;
        self
    }

    /// Borrow the optional P0.8 scheduler service.
    #[must_use]
    pub(crate) fn scheduler(&self) -> Option<&Arc<SchedulerService>> {
        self.products.scheduler.as_ref()
    }

    /// Attach the P3.3 cache and Images operator authority.
    #[must_use]
    pub(crate) fn with_cache_images_api(mut self, api: CacheImagesApiState) -> Self {
        self.products.cache_images_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional P3.3 operator authority.
    #[must_use]
    pub(crate) fn cache_images_api(&self) -> Option<&Arc<CacheImagesApiState>> {
        self.products.cache_images_api.as_ref()
    }

    /// Borrow the fixed-series metrics registry from product control handlers.
    #[must_use]
    pub(crate) const fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.platform.metrics
    }

    /// Attach Vectorize and AI Search operator lifecycle authority.
    #[must_use]
    pub fn with_search_api(mut self, api: SearchApiState) -> Self {
        self.products.search_api = Some(Arc::new(api));
        self
    }

    #[must_use]
    pub(crate) fn with_browser(
        mut self,
        browser: Option<Arc<crate::browser::BrowserService>>,
    ) -> Self {
        self.products.browser = browser;
        self
    }

    pub(crate) fn browser_service(&self) -> Option<&Arc<crate::browser::BrowserService>> {
        self.products.browser.as_ref()
    }

    pub(crate) fn search_api(&self) -> Option<&Arc<SearchApiState>> {
        self.products.search_api.as_ref()
    }

    /// Borrow the resolved admin capability without exposing its value.
    #[must_use]
    pub(crate) fn admin_secret(&self) -> Option<&SecretString> {
        self.auth.admin_secret.as_deref()
    }

    /// Borrow the resolved deployer capability without exposing its value.
    #[must_use]
    pub(crate) fn deployer_secret(&self) -> Option<&SecretString> {
        self.auth.deployer_secret.as_deref()
    }

    /// Borrow the resolved read-only capability without exposing its value.
    #[must_use]
    pub(crate) fn read_only_secret(&self) -> Option<&SecretString> {
        self.auth.read_only_secret.as_deref()
    }

    /// Attach three distinct v4 Bearer capabilities in crate-local tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_v4_tokens(
        mut self,
        deployer: SecretString,
        read_only: SecretString,
    ) -> Self {
        self.auth.deployer_secret = Some(Arc::new(deployer));
        self.auth.read_only_secret = Some(Arc::new(read_only));
        self
    }

    /// Attach the Cloudflare v4 view of one instance for focused tests.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_v4_instance_context(mut self, authority: V4InstanceContext) -> Self {
        self.products.v4_instance_context = Some(Arc::new(authority));
        self
    }

    /// Borrow the Cloudflare v4 view of this instance.
    #[must_use]
    pub(crate) fn v4_instance_context(&self) -> Option<&V4InstanceContext> {
        self.products.v4_instance_context.as_deref()
    }

    /// Attach one instance's storage and Cloudflare v4 view.
    #[must_use]
    pub fn with_platform_storage(mut self, storage: Arc<PlatformStorage>) -> Self {
        if self.products.v4_instance_context.is_none() {
            self.products.v4_instance_context = Some(Arc::new(V4InstanceContext::new(
                storage.identity().instance_id,
                storage.identity().created_at_ms,
            )));
        }
        self.products.platform_storage = Some(storage);
        self
    }

    /// Borrow the one platform persistence authority.
    #[must_use]
    pub(crate) fn platform_storage(&self) -> Option<&Arc<PlatformStorage>> {
        self.products.platform_storage.as_ref()
    }
}
