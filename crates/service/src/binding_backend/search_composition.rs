//! Search-product composition entry points for the private binding backend.

use super::{KvBindingExecutor, serve_binding_backend_inner};
use crate::d1_backend::D1BindingService;
use crate::metrics::MetricsRegistry;
use crate::r2_backend::R2BindingService;
use open_compute_core::{DurableObjectsConfig, PlatformError, QueuesConfig};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{PlatformStorage, SchedulerStore};
use open_compute_workers::ResourcePins;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Serve every product plane, including version-scoped Markdown Conversion.
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend_with_document_parser(
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
    document_parser: Arc<crate::document_parser_backend::DocumentParserBindingService>,
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
        Some(document_parser),
        None,
        None,
        shutdown,
    )
    .await
}

/// Serve every product plane, including Markdown Conversion and AI Search,
/// for an isolated environment without retained platform snapshots.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend_with_ai_search(
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
    document_parser: Arc<crate::document_parser_backend::DocumentParserBindingService>,
    ai: open_compute_core::AiConfig,
    ai_search_objects: open_compute_artifacts::AiSearchObjectStore,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let ai_search = Arc::new(
        crate::ai_search_backend::AiSearchBindingService::new(
            storage.clone(),
            pins.clone(),
            ai,
            ai_search_objects,
            Arc::new(crate::snapshot_pins::SnapshotPins::empty()),
            document_parser.clone(),
        )?
        .with_metrics_opt(metrics.clone()),
    );
    serve_binding_backend_with_ai_search_and_snapshot_pins(
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
        assets,
        services,
        cache,
        images,
        document_parser,
        ai_search,
        None,
        shutdown,
    )
    .await
}

/// Production AI Search composition with authenticated snapshot pins.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_binding_backend_with_ai_search_and_snapshot_pins(
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
    document_parser: Arc<crate::document_parser_backend::DocumentParserBindingService>,
    ai_search: Arc<crate::ai_search_backend::AiSearchBindingService>,
    health: Option<crate::health::HealthCoordinator>,
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
        Some(document_parser),
        Some(ai_search),
        health,
        shutdown,
    )
    .await
}
