use super::*;

pub(super) struct BoundPlatform {
    pub(super) loaded: LoadedConfig,
    pub(super) opts: RunInner,
    pub(super) metrics: Arc<MetricsRegistry>,
    pub(super) health: HealthCoordinator,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) scheduler_store: Arc<open_compute_storage::SchedulerStore>,
    pub(super) observability: Arc<ObservabilityService>,
    pub(super) cache: Arc<ArtifactCache>,
    pub(super) response_cache: Arc<CacheBindingService>,
    pub(super) images: Arc<ImageBindingService>,
    pub(super) document_parser: Arc<DocumentParserBindingService>,
    pub(super) generation_auth: GenerationAuthRegistry,
    pub(super) binding_generation_auth: GenerationAuthRegistry,
    pub(super) observability_generation_auth: GenerationAuthRegistry,
    pub(super) runtime_source_listener: tokio::net::TcpListener,
    pub(super) runtime_source_addr: SocketAddr,
    pub(super) binding_backend_listener: tokio::net::TcpListener,
    pub(super) binding_backend_addr: SocketAddr,
    pub(super) observability_backend_listener: tokio::net::TcpListener,
    pub(super) observability_backend_addr: SocketAddr,
    pub(super) compiler: StaticConfigCompiler,
    pub(super) store: ArtifactStore,
    pub(super) redactor: Redactor,
    pub(super) runtime: open_compute_runtime::VerifiedRuntime,
    pub(super) runtime_lease_path: std::path::PathBuf,
    pub(super) durable_object_storage: std::path::PathBuf,
    pub(super) merged: bool,
    pub(super) version_pins: VersionPins,
    pub(super) service_invocations: Arc<ServiceInvocationRegistry>,
    pub(super) supervisor_handle: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
    pub(super) transport: WorkerdTransport,
    pub(super) scheduler_service: Arc<SchedulerService>,
    pub(super) bundle_limits: BundleLimits,
    pub(super) resource_pins: ResourcePins,
    pub(super) r2_backend: Arc<R2BindingService>,
    pub(super) d1_backend: Arc<D1BindingService>,
    pub(super) binding_executor: Arc<SqliteKvBindingExecutor>,
    pub(super) binding_ai_search: Arc<AiSearchBindingService>,
    pub(super) dashboard_dispatch: Arc<RwLock<Option<crate::dashboard::DashboardDispatch>>>,
    pub(super) state: HttpState,
    pub(super) public_listener: tokio::net::TcpListener,
    pub(super) admin_listener: Option<tokio::net::TcpListener>,
    pub(super) shutdown_tx: watch::Sender<bool>,
    pub(super) shutdown_rx: watch::Receiver<bool>,
    pub(super) scheduler_shutdown_tx: watch::Sender<bool>,
    pub(super) scheduler_shutdown_rx: watch::Receiver<bool>,
    pub(super) control_descriptor: crate::instance_control::GenerationDescriptor,
    pub(super) control_update_tx:
        mpsc::UnboundedSender<crate::instance_control::GenerationDescriptor>,
    pub(super) control_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    pub(super) maintenance_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
}

pub(super) async fn run(platform: BoundPlatform) -> Result<(), PlatformError> {
    let BoundPlatform {
        loaded,
        opts,
        metrics,
        health,
        storage,
        scheduler_store,
        observability,
        cache,
        response_cache,
        images,
        document_parser,
        generation_auth,
        binding_generation_auth,
        observability_generation_auth,
        runtime_source_listener,
        runtime_source_addr,
        binding_backend_listener,
        binding_backend_addr,
        observability_backend_listener,
        observability_backend_addr,
        compiler,
        store,
        redactor,
        runtime,
        runtime_lease_path,
        durable_object_storage,
        merged,
        version_pins,
        service_invocations,
        supervisor_handle,
        transport,
        scheduler_service,
        bundle_limits,
        resource_pins,
        r2_backend,
        d1_backend,
        binding_executor,
        binding_ai_search,
        dashboard_dispatch,
        state,
        public_listener,
        admin_listener,
        shutdown_tx,
        shutdown_rx,
        scheduler_shutdown_tx,
        scheduler_shutdown_rx,
        control_descriptor,
        control_update_tx,
        control_task,
        maintenance_task,
    } = platform;

    let runtime_source = RuntimeSource::new(storage.clone(), store.clone(), bundle_limits)
        .with_cache(cache.clone())
        .with_cache_fail_open(loaded.config.response_cache.fail_open);
    let mut shutdown_source = shutdown_rx.clone();
    let source_auth = generation_auth.clone();
    let runtime_source_task = tokio::spawn(async move {
        serve_runtime_source(
            runtime_source_listener,
            runtime_source,
            source_auth,
            async move {
                let _ = shutdown_source.changed().await;
            },
        )
        .await
    });
    let mut shutdown_binding = shutdown_rx.clone();
    let binding_storage = storage.clone();
    let binding_auth = binding_generation_auth.clone();
    let binding_metrics = metrics.clone();
    let binding_do_config = loaded.config.durable_objects.clone();
    let binding_queue_config = loaded.config.queues.clone();
    let binding_workflow_config = loaded.config.workflows.clone();
    let binding_assets = Arc::new(AssetBindingService::new(
        storage.clone(),
        store.clone(),
        cache.clone(),
        version_pins.clone(),
    ));
    let binding_service_invocations = service_invocations.clone();
    let binding_images = images.clone();
    let binding_document_parser = document_parser.clone();
    let binding_health = health.clone();
    let binding_backend_task = tokio::spawn(async move {
        serve_binding_backend_with_ai_search_and_snapshot_pins(
            binding_backend_listener,
            binding_storage,
            binding_auth,
            resource_pins,
            binding_executor,
            Some(binding_metrics),
            Some(r2_backend),
            Some(d1_backend),
            binding_do_config,
            binding_queue_config,
            binding_workflow_config,
            Some(scheduler_store),
            binding_assets,
            binding_service_invocations,
            Some(response_cache),
            Some(binding_images),
            binding_document_parser,
            binding_ai_search,
            Some(binding_health),
            async move {
                let _ = shutdown_binding.changed().await;
            },
        )
        .await
    });
    let mut shutdown_observability = shutdown_rx.clone();
    let observability_backend_service = observability.clone();
    let observability_backend_auth = observability_generation_auth.clone();
    let observability_backend_task = tokio::spawn(async move {
        serve_observability_backend(
            observability_backend_listener,
            observability_backend_service,
            observability_backend_auth,
            async move {
                let _ = shutdown_observability.changed().await;
            },
        )
        .await
    });
    let public_router = if merged {
        http::merged_router(state.clone())
    } else {
        http::public_router(state.clone())
    };
    let mut shutdown_public = shutdown_rx.clone();
    let public_task = tokio::spawn(async move {
        http::serve_until(public_listener, public_router, async move {
            let _ = shutdown_public.changed().await;
        })
        .await
    });
    let admin_task = if let Some(listener) = admin_listener {
        let router = http::admin_router(state.clone());
        let mut rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            http::serve_until(listener, router, async move {
                let _ = rx.changed().await;
            })
            .await
        }))
    } else {
        None
    };

    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: loaded.config.runtime.clone(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor,
            lease_path: Some(runtime_lease_path),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", runtime_source_addr)?,
            ExternalServiceAddress::loopback("binding-backend", binding_backend_addr)?,
            ExternalServiceAddress::loopback("observability-backend", observability_backend_addr)?,
        ],
        vec![DirectoryServicePath::local(
            "do-storage",
            &durable_object_storage,
        )?],
        vec![
            generation_auth,
            binding_generation_auth,
            observability_generation_auth,
        ],
    ));
    *supervisor_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supervisor.clone());
    supervisor.start();
    record(&opts, "supervisor");
    metrics.inc_start(StartResult::Success, StartStage::Supervisor);
    if loaded.config.dashboard.enabled {
        let bootstrap_storage = storage.clone();
        let bootstrap_store = store.clone();
        let bootstrap_transport = transport.clone();
        let bootstrap_account = storage.identity().default_account_id;
        let bootstrap_limits = bundle_limits;
        let bootstrap_supervisor = supervisor.clone();
        let bootstrap_slot = dashboard_dispatch.clone();
        tokio::spawn(async move {
            if !wait_for_supervisor_running(&bootstrap_supervisor, Duration::from_secs(120)).await {
                tracing::error!("dashboard bootstrap timed out waiting for workerd readiness");
                return;
            }
            match bootstrap_dashboard(
                bootstrap_storage,
                bootstrap_store,
                bootstrap_transport,
                bootstrap_account,
                bootstrap_limits,
            )
            .await
            {
                Ok(dispatch) => {
                    *bootstrap_slot.write().await = Some(dispatch);
                }
                Err(error) => {
                    tracing::error!(code = error.code().as_str(), "dashboard bootstrap failed");
                }
            }
        });
    }
    let scheduler_task = Some(tokio::spawn(async move {
        scheduler_service.run(scheduler_shutdown_rx).await
    }));

    let mut watch_rx = supervisor.subscribe();
    let health_watch = health.clone();
    let metrics_watch = metrics.clone();
    let storage_watch = storage.clone();
    let service_invocations_watch = service_invocations;
    let version_pins_watch = version_pins.clone();
    let images_watch = images;
    let control_descriptor_watch = control_descriptor;
    tokio::spawn(async move {
        let mut generation_resources = RuntimeGenerationResources::new(
            service_invocations_watch.as_ref().clone(),
            version_pins_watch.clone(),
        );
        loop {
            let snap = watch_rx.borrow().clone();
            metrics_watch.observe_supervisor(&snap);
            let generation_update = generation_resources.observe(&snap);
            if snap.state == SupervisorState::Running
                && generation_update.child_changed
                && DurableObjectRepository::new(&storage_watch)
                    .count_live_objects()
                    .is_ok_and(|count| count > 0)
            {
                metrics_watch.inc_do_facet_reload(DoFacetReloadReason::Restart);
            }
            if generation_update.resources_cleared {
                metrics_watch.set_service_invocation_counts(0, 0, 0);
                if let Err(error) = images_watch.clear_sessions() {
                    tracing::error!(
                        code = error.code().as_str(),
                        "failed to clear image sessions after runtime generation transition"
                    );
                }
            }
            if let Err(err) = health_watch.apply_supervisor(&snap) {
                tracing::error!(
                    code = err.code().as_str(),
                    "runtime health transition failed"
                );
            }
            let mut descriptor = control_descriptor_watch.clone();
            descriptor.readiness = match snap.state {
                SupervisorState::Running => "ready",
                SupervisorState::BackingOff => "degraded",
                SupervisorState::Failed => "failed",
                _ => "starting",
            }
            .to_owned();
            descriptor.published_at = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                .unwrap_or(u64::MAX);
            let _ = control_update_tx.send(descriptor);
            if watch_rx.changed().await.is_err() {
                break;
            }
        }
    });

    let run_err = wait_signals_and_servers(
        &health,
        &supervisor,
        shutdown_tx,
        scheduler_shutdown_tx,
        public_task,
        admin_task,
        runtime_source_task,
        binding_backend_task,
        observability_backend_task,
        control_task,
        maintenance_task,
        scheduler_task,
    )
    .await;
    drop(cache);
    drop(store);
    drop(storage);
    match run_err {
        None => Ok(()),
        Some(err) => Err(err),
    }
}
