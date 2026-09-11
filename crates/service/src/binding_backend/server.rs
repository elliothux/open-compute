use super::*;

/// Bind the private binding backend to an ephemeral IPv4 loopback port.
pub async fn bind_binding_backend() -> Result<TcpListener, PlatformError> {
    TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "failed to bind private binding backend listener",
            )
        })
}

/// Serve every composed product plane on the private binding listener.
#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub async fn serve_binding_backend(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_inner(
        listener,
        storage,
        auth,
        pins,
        executor,
        metrics,
        r2,
        d1,
        do_config,
        queue_config,
        workflow_config,
        scheduler,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        shutdown,
    )
    .await
}

/// Serve every product plane plus the version-scoped static-assets binding backend.
#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub async fn serve_binding_backend_with_assets(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    assets: Arc<crate::asset_backend::AssetBindingService>,
    services: Arc<crate::service_invocations::ServiceInvocationRegistry>,
    cache: Option<Arc<crate::cache_backend::CacheBindingService>>,
    images: Option<Arc<crate::images_backend::ImageBindingService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_inner(
        listener,
        storage,
        auth,
        pins,
        executor,
        metrics,
        r2,
        d1,
        do_config,
        queue_config,
        workflow_config,
        scheduler,
        Some(assets),
        Some(services),
        cache,
        images,
        None,
        None,
        None,
        None,
        shutdown,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub(super) async fn serve_binding_backend_inner(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    assets: Option<Arc<crate::asset_backend::AssetBindingService>>,
    services: Option<Arc<crate::service_invocations::ServiceInvocationRegistry>>,
    cache: Option<Arc<crate::cache_backend::CacheBindingService>>,
    images: Option<Arc<crate::images_backend::ImageBindingService>>,
    document_parser: Option<Arc<crate::document_parser_backend::DocumentParserBindingService>>,
    ai_search: Option<Arc<crate::ai_search_backend::AiSearchBindingService>>,
    artifacts: Option<Arc<crate::artifact_api::ArtifactApiState>>,
    health: Option<crate::health::HealthCoordinator>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let (global_streams, resource_streams) = executor.stream_limits();
    let queue = scheduler.as_ref().map(|scheduler| {
        let service = QueueBindingService::new(storage.clone(), scheduler.clone())
            .with_concurrency_limits(
                queue_config.max_in_flight_requests,
                queue_config.max_in_flight_requests_per_binding,
            );
        Arc::new(match &metrics {
            Some(metrics) => service.with_metrics(metrics.clone()),
            None => service,
        })
    });
    let workflow = scheduler
        .as_ref()
        .map(|scheduler| {
            crate::workflow_backend::WorkflowBindingService::new(
                storage.clone(),
                scheduler.clone(),
                workflow_config,
            )
            .map(|service| match &metrics {
                Some(metrics) => service.with_metrics(metrics.clone()),
                None => service,
            })
        })
        .transpose()?
        .map(Arc::new);
    let vectorize_coordinator =
        crate::vectorize_coordinator::VectorizeCoordinator::new(storage.clone(), pins.clone());
    let vectorize_coordinator = match &metrics {
        Some(metrics) => vectorize_coordinator.with_metrics(metrics.clone()),
        None => vectorize_coordinator,
    };
    let vectorize_coordinator = match &health {
        Some(health) => vectorize_coordinator.with_health(health.clone()),
        None => vectorize_coordinator,
    };
    let service_reaper = services.clone();
    let ai_search_maintenance = ai_search.clone();
    let state = BackendState {
        storage,
        auth,
        pins,
        executor,
        metrics,
        stream_budget: StreamBudget::new(global_streams, resource_streams),
        r2,
        d1,
        do_config,
        scheduler,
        queue,
        workflow,
        assets,
        services,
        cache,
        images,
        document_parser,
        ai_search,
        artifacts,
    };
    let router = Router::new().fallback(handle).with_state(state);
    let (vectorize_shutdown, vectorize_shutdown_rx) = tokio::sync::watch::channel(false);
    let vectorize_task = tokio::spawn(vectorize_coordinator.run(vectorize_shutdown_rx));
    let (ai_search_shutdown, mut ai_search_shutdown_rx) = tokio::sync::watch::channel(false);
    let ai_search_task = tokio::spawn(async move {
        let Some(service) = ai_search_maintenance else {
            return;
        };
        let publish_health = |healthy| {
            if let Some(health) = &health {
                let _ = health.set_search_background(
                    open_compute_core::ComponentName::AiSearchIndexing,
                    healthy,
                );
            }
        };
        publish_health(service.maintenance_once().await.is_ok());
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = ai_search_shutdown_rx.changed() => {
                    if changed.is_err() || *ai_search_shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    publish_health(service.maintenance_once().await.is_ok());
                }
            }
        }
    });
    let managed_shutdown = async move {
        match service_reaper {
            Some(registry) => {
                registry
                    .reap_deadlines_until_shutdown(
                        crate::service_invocations::DEADLINE_REAPER_INTERVAL,
                        shutdown,
                    )
                    .await;
            }
            None => shutdown.await,
        }
        let _ = vectorize_shutdown.send(true);
        let _ = ai_search_shutdown.send(true);
        let _ = vectorize_task.await;
        let _ = ai_search_task.await;
    };
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(managed_shutdown)
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "private binding backend listener failed",
            )
        })
}
