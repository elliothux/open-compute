use super::*;

mod bound;
mod composition;
use super::startup::PreparedPlatform;

pub(super) async fn run_prepared(prepared: PreparedPlatform) -> Result<(), PlatformError> {
    serve(Box::pin(composition::compose(prepared)).await?).await
}

fn spawn_instance_control(
    mut control: crate::instance_control::InstanceControl,
    mut updates: mpsc::UnboundedReceiver<crate::instance_control::GenerationDescriptor>,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<Result<(), PlatformError>> {
    tokio::spawn(async move {
        let mut updates_open = true;
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                update = updates.recv(), if updates_open => {
                    if let Some(descriptor) = update {
                        control.update_descriptor(descriptor)?;
                    } else {
                        updates_open = false;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => control.poll_once()?,
            }
        }
        Ok(())
    })
}

#[cfg(feature = "test-support")]
fn with_test_runtime_restart(
    state: HttpState,
    supervisor_handle: &Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
) -> HttpState {
    let supervisor_for_restart = supervisor_handle.clone();
    state.with_test_runtime_restart(Arc::new(move || {
        let Ok(supervisor) = supervisor_for_restart.lock() else {
            return false;
        };
        let Some(supervisor) = supervisor.as_ref() else {
            return false;
        };
        supervisor.force_restart_for_test();
        true
    }))
}

async fn serve(composed: composition::ComposedPlatform) -> Result<(), PlatformError> {
    let composition::ComposedPlatform {
        loaded,
        opts,
        metrics,
        health,
        storage,
        scheduler_store,
        observability,
        cache,
        response_cache,
        response_cache_manager,
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
        maintenance_backend,
        maintenance_object_storage,
        r2_objects,
        store,
        snapshot_pins,
        redactor,
        runtime,
        runtime_lease_path,
        durable_object_storage,
        public_addr,
        admin_addr,
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
        maintenance_do_lifecycle,
        binding_executor,
        binding_ai_search,
        dashboard_dispatch,
        generation_startup_id,
        dashboard_auth,
        state,
    } = composed;

    #[cfg(feature = "test-support")]
    let state = with_test_runtime_restart(state, &supervisor_handle);

    let state = state.with_local_origin_addr(public_addr);
    record(&opts, "listen");
    #[cfg(any(test, feature = "test-support"))]
    if let Err(err) = fail_after(&opts, FailAfter::Listen, &metrics, StartStage::Listen) {
        drop(cache);
        drop(store);
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::Listen);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = watch::channel(false);

    let instance_id = storage.identity().instance_id;
    if let Some(api) = &opts.daemon_api {
        api.register_cache(instance_id, &cache, &storage)?;
    }
    let control_scope = opts
        .scope
        .ok_or_else(|| PlatformError::new(ErrorCode::ConfigInvalid, "OCD scope is missing"))?;
    let scoped_run_root = opts
        .instance_registry
        .as_ref()
        .map(|registry| registry.root_for(control_scope).join("run"));
    let control_root = crate::instance_control::runtime_dir_for(
        control_scope,
        &instance_id,
        scoped_run_root.as_deref(),
    )?;
    let public_bound = Some(public_addr.to_string());
    let admin_bound = admin_addr.map(|addr| addr.to_string());
    let control_descriptor = crate::instance_control::build_descriptor(
        &instance_id,
        &loaded.path,
        generation_startup_id,
        env!("CARGO_PKG_VERSION"),
        control_scope,
        public_bound,
        admin_bound,
        "starting",
        SystemTime::now(),
    )?;
    let instance_control = crate::instance_control::InstanceControl::publish(
        &control_root,
        control_descriptor.clone(),
        shutdown_tx.clone(),
        dashboard_auth,
    )?;
    let state = match (&loaded.config.public_gateway, &opts.gateway_pids) {
        (Some(_), Some((child_pid, qualified_pid))) => {
            state.with_public_gateway_process(child_pid.clone(), qualified_pid.clone())
        }
        (Some(_), None) => {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "public domain requires the shared Gateway",
            ));
        }
        (None, _) => state,
    };
    let (control_update_tx, control_update_rx) = mpsc::unbounded_channel();
    let control_task =
        spawn_instance_control(instance_control, control_update_rx, shutdown_rx.clone());
    let mut shutdown_maintenance = shutdown_rx.clone();
    let maintenance_storage = storage.clone();
    let maintenance_store = store.clone();
    let maintenance_cache = cache.clone();
    let maintenance_response_cache = response_cache_manager.clone();
    let maintenance_config = loaded.config.workers.clone();
    let maintenance_kv_config = loaded.config.kv.clone();
    let maintenance_r2_config = loaded.config.r2.clone();
    let maintenance_do_config = loaded.config.durable_objects.clone();
    let maintenance_snapshot_stale_after_ms = loaded.config.hardening.snapshot_stale_after_ms;
    let maintenance_emergency_reserve_bytes = loaded.config.hardening.emergency_reserve_bytes;
    let maintenance_r2_objects = r2_objects;
    let maintenance_health = health.clone();
    let maintenance_pins = version_pins.clone();
    let maintenance_resource_pins = resource_pins.clone();
    let maintenance_metrics = metrics.clone();
    let maintenance_snapshot_pins = snapshot_pins.clone();
    let maintenance_task = tokio::spawn(async move {
        let mut r2_maintenance = R2Maintenance::default();
        let mut interval = tokio::time::interval(Duration::from_millis(
            maintenance_config.artifact_gc_interval_ms,
        ));
        loop {
            tokio::select! {
                _ = shutdown_maintenance.changed() => return Ok(()),
                _ = interval.tick() => {
                    run_worker_maintenance(
                        &maintenance_storage,
                        &maintenance_store,
                        &maintenance_cache,
                        &maintenance_response_cache,
                        &maintenance_pins,
                        &maintenance_config,
                        &maintenance_snapshot_pins,
                        &maintenance_metrics,
                    ).await;
                    run_kv_maintenance(
                        &maintenance_storage,
                        &maintenance_resource_pins,
                        &maintenance_kv_config,
                        &maintenance_metrics,
                    ).await;
                    r2_maintenance.run(
                        &maintenance_storage,
                        &maintenance_r2_objects,
                        &maintenance_r2_config,
                        &maintenance_health,
                    ).await;
                    let _ = update_local_object_storage_health(
                        &maintenance_backend,
                        &maintenance_object_storage,
                        &maintenance_health,
                    );
                    let _ = update_do_storage_health(
                        &maintenance_storage,
                        &maintenance_do_config,
                        &maintenance_health,
                        &maintenance_metrics,
                    );
                    let _ = update_operations_health(
                        maintenance_storage.data_dir(),
                        maintenance_snapshot_stale_after_ms,
                        &maintenance_health,
                    );
                    if let Err(error) = refresh_p1_metrics(
                        &maintenance_storage,
                        &maintenance_metrics,
                        maintenance_emergency_reserve_bytes,
                    ) {
                        tracing::warn!(
                            code = error.code().as_str(),
                            "P1 disk and resource metrics refresh failed"
                        );
                    }
                    let _ = maintenance_do_lifecycle.reconcile_pending().await;
                }
            }
        }
    });
    let shared_routes = opts
        .shared_routes
        .clone()
        .ok_or_else(|| PlatformError::new(ErrorCode::ConfigInvalid, "shared routes are missing"))?;
    bound::run(bound::BoundPlatform {
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
    })
    .await
}
