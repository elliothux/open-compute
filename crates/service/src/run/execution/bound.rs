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
    pub(super) version_pins: VersionPins,
    pub(super) service_invocations: Arc<ServiceInvocationRegistry>,
    pub(super) host_extension_broker: Arc<HostExtensionBroker>,
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
    pub(super) instance_id: InstanceId,
    pub(super) shared_routes: http::SharedRoutes,
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
        version_pins,
        service_invocations,
        host_extension_broker,
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
        instance_id,
        shared_routes,
        shutdown_tx,
        shutdown_rx,
        scheduler_shutdown_tx,
        scheduler_shutdown_rx,
        control_descriptor,
        control_update_tx,
        control_task,
        maintenance_task,
    } = platform;

    let daemon_status = opts.daemon_api.clone().map(|api| (api, instance_id));
    let route_lease = shared_routes.insert(
        instance_id,
        state.clone(),
        loaded
            .config
            .public_gateway
            .as_ref()
            .map(|config| config.base_domain.as_str()),
    )?;

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
    let binding_artifacts = artifact_api(
        &storage,
        &loaded.config.artifacts,
        opts.artifact_requests()?,
    )?;
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
            Some(binding_artifacts),
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
    let host_extension_socket_registry = host_extension_broker.socket_registry();
    let broker = host_extension_broker.clone();
    let broker_shutdown = shutdown_rx.clone();
    let host_extension_broker_task = tokio::spawn(async move { broker.run(broker_shutdown).await });
    let supervisor = Arc::new(WorkerdSupervisor::new_with_host_extension_broker(
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
        host_extension_socket_registry,
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
        let bootstrap_account = storage.identity().instance_id;
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

    spawn_supervisor_watch(SupervisorWatch {
        receiver: supervisor.subscribe(),
        health: health.clone(),
        metrics: metrics.clone(),
        storage: storage.clone(),
        service_invocations,
        version_pins: version_pins.clone(),
        images,
        descriptor: control_descriptor,
        diagnostics_root: loaded.config.data.path.clone(),
        supervisor: supervisor.clone(),
        control_update_tx,
        daemon_status,
    });

    let daemon_shutdown = opts.shutdown.clone().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "instance shutdown channel is missing",
        )
    })?;
    let run_err = wait_instance_and_servers(
        &health,
        &supervisor,
        daemon_shutdown,
        shutdown_rx,
        route_lease,
        shutdown_tx,
        scheduler_shutdown_tx,
        runtime_source_task,
        binding_backend_task,
        observability_backend_task,
        host_extension_broker_task,
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

struct SupervisorWatch {
    receiver: watch::Receiver<open_compute_runtime::supervisor::SupervisorSnapshot>,
    health: HealthCoordinator,
    metrics: Arc<MetricsRegistry>,
    storage: Arc<PlatformStorage>,
    service_invocations: Arc<ServiceInvocationRegistry>,
    version_pins: VersionPins,
    images: Arc<ImageBindingService>,
    descriptor: crate::instance_control::GenerationDescriptor,
    diagnostics_root: std::path::PathBuf,
    supervisor: Arc<WorkerdSupervisor>,
    control_update_tx: mpsc::UnboundedSender<crate::instance_control::GenerationDescriptor>,
    daemon_status: Option<(daemon_control::DaemonApi, InstanceId)>,
}

fn spawn_supervisor_watch(mut watch: SupervisorWatch) {
    tokio::spawn(async move {
        let mut generation_resources = RuntimeGenerationResources::new(
            watch.service_invocations.as_ref().clone(),
            watch.version_pins.clone(),
        );
        let mut recorded_startup_id = None;
        loop {
            let snapshot = watch.receiver.borrow().clone();
            if let (Some(startup_id), Some(exit), Some(diagnostics)) = (
                snapshot.last_exit_startup_id,
                snapshot.last_exit.as_ref(),
                watch.supervisor.last_diagnostics(),
            ) && recorded_startup_id != Some(startup_id)
            {
                let timestamp_ms = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                    .unwrap_or(i64::MAX);
                let mut deployments = watch.version_pins.active_deployments();
                deployments.sort_unstable_by_key(ToString::to_string);
                deployments.dedup();
                let attribution = if exit.code_name != ErrorCode::RuntimeExitedInFlight.as_str() {
                    "not_applicable"
                } else if let [deployment_id] = deployments.as_slice() {
                    match WorkerRepository::new(watch.storage.db()).quarantine_active_deployment(
                        *deployment_id,
                        "RUNTIME_UNEXPECTED_EXIT",
                        RequestId::generate(),
                        timestamp_ms,
                    ) {
                        Ok(true) => "deployment_quarantined",
                        Ok(false) => "attribution_stale",
                        Err(error) => {
                            tracing::error!(
                                code = error.code().as_str(),
                                "failed to quarantine attributed deployment"
                            );
                            "attribution_failed"
                        }
                    }
                } else if deployments.len() > 1 {
                    "attribution_ambiguous"
                } else {
                    "unattributed"
                };
                if let Err(error) = crate::runtime_diagnostics::record(
                    &watch.diagnostics_root,
                    timestamp_ms,
                    startup_id,
                    exit,
                    &diagnostics,
                    attribution,
                ) {
                    tracing::error!(
                        code = error.code().as_str(),
                        "failed to persist workerd incident diagnostics"
                    );
                } else {
                    recorded_startup_id = Some(startup_id);
                }
            }
            watch.metrics.observe_supervisor(&snapshot);
            let generation_update = generation_resources.observe(&snapshot);
            if snapshot.state == SupervisorState::Running
                && generation_update.child_changed
                && DurableObjectRepository::new(&watch.storage)
                    .count_live_objects()
                    .is_ok_and(|count| count > 0)
            {
                watch
                    .metrics
                    .inc_do_facet_reload(DoFacetReloadReason::Restart);
            }
            if generation_update.resources_cleared {
                watch.metrics.set_service_invocation_counts(0, 0, 0);
                if let Err(error) = watch.images.clear_sessions() {
                    tracing::error!(
                        code = error.code().as_str(),
                        "failed to clear image sessions after runtime generation transition"
                    );
                }
            }
            if let Err(error) = watch.health.apply_supervisor(&snapshot) {
                tracing::error!(
                    code = error.code().as_str(),
                    "runtime health transition failed"
                );
            }
            if let Some((api, id)) = &watch.daemon_status {
                match snapshot.state {
                    SupervisorState::Running => {
                        let _ = api.mark(id, "running", None);
                    }
                    SupervisorState::Failed => {
                        let _ = api.mark(id, "failed", Some(ErrorCode::RuntimeUnavailable));
                    }
                    _ => {}
                }
            }
            let mut descriptor = watch.descriptor.clone();
            descriptor.readiness = match snapshot.state {
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
            let _ = watch.control_update_tx.send(descriptor);
            if watch.receiver.changed().await.is_err() {
                break;
            }
        }
    });
}

fn artifact_api(
    storage: &Arc<PlatformStorage>,
    config: &open_compute_core::ArtifactsConfig,
    requests: Arc<Semaphore>,
) -> Result<Arc<crate::artifact_api::ArtifactApiState>, PlatformError> {
    crate::artifact_api::ArtifactApiState::new(Arc::clone(storage), config.clone(), requests)
        .map(Arc::new)
}
