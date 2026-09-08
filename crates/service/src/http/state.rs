use super::*;

/// Shared HTTP transport state composed by the service root.
#[derive(Clone)]
pub struct HttpState {
    pub(super) health: HealthCoordinator,
    pub(super) metrics: Arc<MetricsRegistry>,
    pub(super) metrics_enabled: bool,
    pub(super) dashboard_enabled: bool,
    pub(super) admin_secret: Option<Arc<SecretString>>,
    pub(super) deployer_secret: Option<Arc<SecretString>>,
    pub(super) read_only_secret: Option<Arc<SecretString>>,
    pub(super) cloudflare_v4_account: Option<Arc<AccountAuthority>>,
    pub(super) platform_storage: Option<Arc<PlatformStorage>>,
    #[cfg(any(test, feature = "test-support"))]
    pub(super) test_runtime_restart: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub(super) worker_api: Option<Arc<WorkerApiState>>,
    pub(super) kv_api: Option<Arc<KvApiState>>,
    pub(super) r2_api: Option<Arc<R2ApiState>>,
    pub(super) d1_api: Option<Arc<D1ApiState>>,
    pub(super) queue_api: Option<Arc<QueueApiState>>,
    pub(super) workflow_api: Option<Arc<WorkflowApiState>>,
    pub(super) scheduler: Option<Arc<SchedulerService>>,
    pub(super) cache_images_api: Option<Arc<CacheImagesApiState>>,
    pub(super) dashboard_dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    pub(super) search_api: Option<Arc<SearchApiState>>,
    pub(super) dashboard_auth: Option<Arc<DashboardAuth>>,
}

impl std::fmt::Debug for HttpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpState")
            .field("metrics_enabled", &self.metrics_enabled)
            .field("dashboard_enabled", &self.dashboard_enabled)
            .field("admin_auth", &self.admin_secret.is_some())
            .field("deployer_auth", &self.deployer_secret.is_some())
            .field("read_only_auth", &self.read_only_secret.is_some())
            .field(
                "cloudflare_v4_account",
                &self.cloudflare_v4_account.is_some(),
            )
            .field("platform_storage", &self.platform_storage.is_some())
            .field(
                "test_runtime_restart",
                &cfg!(any(test, feature = "test-support")),
            )
            .field("worker_api", &self.worker_api.is_some())
            .field("kv_api", &self.kv_api.is_some())
            .field("r2_api", &self.r2_api.is_some())
            .field("d1_api", &self.d1_api.is_some())
            .field("queue_api", &self.queue_api.is_some())
            .field("workflow_api", &self.workflow_api.is_some())
            .field("scheduler", &self.scheduler.is_some())
            .field("cache_images_api", &self.cache_images_api.is_some())
            .field("dashboard_dispatch", &"<async>")
            .field("search_api", &self.search_api.is_some())
            .field("dashboard_auth", &self.dashboard_auth.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpState {
    /// Build state. Resolves admin auth when configured.
    pub fn new(
        health: HealthCoordinator,
        metrics: Arc<MetricsRegistry>,
        metrics_enabled: bool,
        dashboard_enabled: bool,
        server: &ServerConfig,
    ) -> Result<Self, PlatformError> {
        let admin_secret = Arc::new(resolve_admin_auth(&server.admin_auth)?);
        let deployer_secret = Arc::new(resolve_bearer_auth(&server.deployer_auth)?);
        let read_only_secret = Arc::new(resolve_bearer_auth(&server.read_only_auth)?);
        if admin_secret.expose() == deployer_secret.expose()
            || admin_secret.expose() == read_only_secret.expose()
            || deployer_secret.expose() == read_only_secret.expose()
        {
            return Err(PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "server Bearer tokens must be distinct",
            ));
        }
        Ok(Self {
            health,
            metrics,
            metrics_enabled,
            dashboard_enabled,
            admin_secret: Some(admin_secret),
            deployer_secret: Some(deployer_secret),
            read_only_secret: Some(read_only_secret),
            cloudflare_v4_account: None,
            platform_storage: None,
            #[cfg(any(test, feature = "test-support"))]
            test_runtime_restart: None,
            worker_api: None,
            kv_api: None,
            r2_api: None,
            d1_api: None,
            queue_api: None,
            workflow_api: None,
            scheduler: None,
            cache_images_api: None,
            dashboard_dispatch: Arc::new(RwLock::new(None)),
            search_api: None,
            dashboard_auth: None,
        })
    }

    /// Attach Dashboard one-time login and short browser session authority.
    #[must_use]
    pub fn with_dashboard_auth(mut self, auth: Arc<DashboardAuth>) -> Self {
        self.dashboard_auth = Some(auth);
        self
    }

    /// Borrow Dashboard auth when this process generation enabled it.
    #[must_use]
    pub(crate) fn dashboard_auth(&self) -> Option<&DashboardAuth> {
        self.dashboard_auth.as_deref()
    }

    /// Share the dashboard dispatch slot populated after runtime bootstrap.
    #[must_use]
    pub fn with_dashboard_dispatch(
        mut self,
        dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    ) -> Self {
        self.dashboard_dispatch = dispatch;
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
            health,
            metrics,
            metrics_enabled,
            dashboard_enabled: false,
            admin_secret: admin_secret.map(Arc::new),
            deployer_secret: None,
            read_only_secret: None,
            cloudflare_v4_account: None,
            platform_storage: None,
            test_runtime_restart: None,
            worker_api: None,
            kv_api: None,
            r2_api: None,
            d1_api: None,
            queue_api: None,
            workflow_api: None,
            scheduler: None,
            cache_images_api: None,
            dashboard_dispatch: Arc::new(RwLock::new(None)),
            search_api: None,
            dashboard_auth: None,
        }
    }

    /// Enable dashboard surface responses in tests without runtime bootstrap.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_dashboard_enabled(mut self, enabled: bool) -> Self {
        self.dashboard_enabled = enabled;
        self
    }

    /// Attach the P0.2 control/data plane to this listener state.
    #[must_use]
    pub fn with_worker_api(mut self, worker_api: WorkerApiState) -> Self {
        self.worker_api = Some(Arc::new(worker_api));
        self
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
        self.worker_api.as_ref()
    }

    /// Attach the P0.4 KV control plane to this listener state.
    #[must_use]
    pub fn with_kv_api(mut self, kv_api: KvApiState) -> Self {
        self.kv_api = Some(Arc::new(kv_api));
        self
    }

    /// Borrow the optional P0.4 API state.
    #[must_use]
    pub(crate) fn kv_api(&self) -> Option<&Arc<KvApiState>> {
        self.kv_api.as_ref()
    }

    /// Attach the P0.5 R2 logical-bucket control plane.
    #[must_use]
    pub fn with_r2_api(mut self, r2_api: R2ApiState) -> Self {
        self.r2_api = Some(Arc::new(r2_api));
        self
    }

    /// Borrow the optional P0.5 R2 control-plane state.
    #[must_use]
    pub(crate) fn r2_api(&self) -> Option<&Arc<R2ApiState>> {
        self.r2_api.as_ref()
    }

    /// Attach the P0.6 D1 control plane.
    #[must_use]
    pub fn with_d1_api(mut self, d1_api: D1ApiState) -> Self {
        self.d1_api = Some(Arc::new(d1_api));
        self
    }

    /// Borrow the optional P0.6 D1 control-plane state.
    #[must_use]
    pub(crate) fn d1_api(&self) -> Option<&Arc<D1ApiState>> {
        self.d1_api.as_ref()
    }

    /// Attach the P2.2 Queue catalog control plane.
    #[must_use]
    pub fn with_queue_api(mut self, queue_api: Option<QueueApiState>) -> Self {
        self.queue_api = queue_api.map(Arc::new);
        self
    }

    /// Borrow the optional P2.2 Queue control-plane state.
    #[must_use]
    pub(crate) fn queue_api(&self) -> Option<&Arc<QueueApiState>> {
        self.queue_api.as_ref()
    }

    /// Attach the Workflow catalog and bounded operator history.
    #[must_use]
    pub fn with_workflow_api(mut self, workflow_api: Option<WorkflowApiState>) -> Self {
        self.workflow_api = workflow_api.map(Arc::new);
        self
    }

    pub(crate) fn workflow_api(&self) -> Option<&Arc<WorkflowApiState>> {
        self.workflow_api.as_ref()
    }

    /// Attach the P0.8 scheduler operator surface.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerService>>) -> Self {
        self.scheduler = scheduler;
        self
    }

    /// Borrow the optional P0.8 scheduler service.
    #[must_use]
    pub(crate) fn scheduler(&self) -> Option<&Arc<SchedulerService>> {
        self.scheduler.as_ref()
    }

    /// Attach the P3.3 cache and Images operator authority.
    #[must_use]
    pub(crate) fn with_cache_images_api(mut self, api: CacheImagesApiState) -> Self {
        self.cache_images_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional P3.3 operator authority.
    #[must_use]
    pub(crate) fn cache_images_api(&self) -> Option<&Arc<CacheImagesApiState>> {
        self.cache_images_api.as_ref()
    }

    /// Borrow the fixed-series metrics registry from product control handlers.
    #[must_use]
    pub(crate) const fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.metrics
    }

    /// Attach Vectorize and AI Search operator lifecycle authority.
    #[must_use]
    pub fn with_search_api(mut self, api: SearchApiState) -> Self {
        self.search_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional Vectorize and AI Search operator authority.
    #[must_use]
    pub(crate) fn search_api(&self) -> Option<&Arc<SearchApiState>> {
        self.search_api.as_ref()
    }

    /// Borrow the resolved admin capability without exposing its value.
    #[must_use]
    pub(crate) fn admin_secret(&self) -> Option<&SecretString> {
        self.admin_secret.as_deref()
    }

    /// Borrow the resolved deployer capability without exposing its value.
    #[must_use]
    pub(crate) fn deployer_secret(&self) -> Option<&SecretString> {
        self.deployer_secret.as_deref()
    }

    /// Borrow the resolved read-only capability without exposing its value.
    #[must_use]
    pub(crate) fn read_only_secret(&self) -> Option<&SecretString> {
        self.read_only_secret.as_deref()
    }

    /// Attach three distinct v4 Bearer capabilities in crate-local tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_v4_tokens(
        mut self,
        deployer: SecretString,
        read_only: SecretString,
    ) -> Self {
        self.deployer_secret = Some(Arc::new(deployer));
        self.read_only_secret = Some(Arc::new(read_only));
        self
    }

    /// Attach the stable one-account Cloudflare v4 identity mapping.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_cloudflare_v4_account(mut self, authority: AccountAuthority) -> Self {
        self.cloudflare_v4_account = Some(Arc::new(authority));
        self
    }

    /// Borrow the stable one-account Cloudflare v4 identity mapping.
    #[must_use]
    pub(crate) fn cloudflare_v4_account(&self) -> Option<&AccountAuthority> {
        self.cloudflare_v4_account.as_deref()
    }

    /// Attach the one platform persistence authority and derive its public v4 account mapping.
    #[must_use]
    pub fn with_platform_storage(mut self, storage: Arc<PlatformStorage>) -> Self {
        if self.cloudflare_v4_account.is_none() {
            self.cloudflare_v4_account = Some(Arc::new(AccountAuthority::new(
                storage.identity().platform_id,
                storage.identity().default_account_id,
                storage.identity().created_at_ms,
            )));
        }
        self.platform_storage = Some(storage);
        self
    }

    /// Borrow the one platform persistence authority.
    #[must_use]
    pub(crate) fn platform_storage(&self) -> Option<&Arc<PlatformStorage>> {
        self.platform_storage.as_ref()
    }
}
