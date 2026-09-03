//! Internal Durable Object object-lifecycle recovery service.
//!
//! Namespace mutation is owned exclusively by Worker `exports`/`migrations`. This module has no
//! public HTTP router or request DTOs; it only resumes crash-safe object create/delete work.

use crate::metrics::{DoFacetReloadReason, DoReconcileState, MetricsRegistry};
use crate::runtime_bridge::WorkerdTransport;
use open_compute_core::{
    AccountId, DurableObjectState, DurableObjectsConfig, ErrorCode, PlatformError,
};
use open_compute_storage::{
    AuthorizedDurableObjectDelete, DurableObjectRecord, DurableObjectRepository, PlatformStorage,
    SchedulerStore,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

trait DurableObjectDeleteTransport: Send + Sync {
    fn delete<'a>(
        &'a self,
        authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>>;
}

impl DurableObjectDeleteTransport for WorkerdTransport {
    fn delete<'a>(
        &'a self,
        authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>> {
        Box::pin(self.delete_durable_object(authority))
    }
}

/// Internal crash-recovery owner for Durable Object object generations.
#[derive(Clone)]
pub struct DurableObjectLifecycleService {
    storage: Arc<PlatformStorage>,
    transport: Arc<dyn DurableObjectDeleteTransport>,
    config: DurableObjectsConfig,
    metrics: Option<Arc<MetricsRegistry>>,
    scheduler: Option<Arc<SchedulerStore>>,
}

impl std::fmt::Debug for DurableObjectLifecycleService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableObjectLifecycleService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DurableObjectLifecycleService {
    /// Bind central authority and the runtime capability needed for fenced native deletes.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        transport: WorkerdTransport,
        config: DurableObjectsConfig,
    ) -> Self {
        Self {
            storage,
            transport: Arc::new(transport),
            config,
            metrics: None,
            scheduler: None,
        }
    }

    /// Record lifecycle activity in the fixed low-cardinality metric registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Delete alarm projections inside the object lifecycle fence before native facet removal.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerStore>>) -> Self {
        self.scheduler = scheduler;
        self
    }

    /// Resume a bounded batch of native object creates/deletes fenced before a crash.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let now_ms = now_ms()?;
        let repository = DurableObjectRepository::new(&self.storage);
        let candidates = repository.reconcile_candidates(self.config.reconcile_batch)?;
        let mut completed = 0_u32;
        for object in candidates {
            let state = object.state;
            let result = match state {
                DurableObjectState::Creating => repository
                    .finish_object_create(
                        object.namespace_resource_id,
                        object.object_id,
                        object.generation,
                        now_ms,
                    )
                    .map(|_| ()),
                DurableObjectState::Deleting => {
                    let namespace =
                        repository.get_namespace_by_resource(object.namespace_resource_id)?;
                    self.delete_fenced_object(namespace.resource.account_id, object, now_ms)
                        .await
                }
                DurableObjectState::Ready | DurableObjectState::Tombstoned => continue,
            };
            if let Some(metrics) = &self.metrics {
                metrics.inc_do_reconcile(
                    match state {
                        DurableObjectState::Creating => DoReconcileState::Creating,
                        DurableObjectState::Deleting => DoReconcileState::Deleting,
                        DurableObjectState::Ready | DurableObjectState::Tombstoned => continue,
                    },
                    result.is_ok(),
                );
            }
            result?;
            completed = completed.saturating_add(1);
        }
        if let Some(metrics) = &self.metrics {
            metrics.set_do_active_hosts(repository.count_live_objects()?);
        }
        Ok(completed)
    }

    async fn delete_fenced_object(
        &self,
        account_id: AccountId,
        object: DurableObjectRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repository = DurableObjectRepository::new(&self.storage);
        let authority = repository.deletion_authority(
            account_id,
            object.namespace_resource_id,
            object.object_id,
            object.generation,
        )?;
        if let Some(scheduler) = &self.scheduler {
            scheduler.delete_object(
                object.namespace_resource_id,
                object.object_id,
                object.generation,
            )?;
        }
        self.transport.delete(&authority).await?;
        if let Some(metrics) = &self.metrics {
            metrics.inc_do_facet_reload(DoFacetReloadReason::Delete);
        }
        repository
            .finish_object_delete(
                object.namespace_resource_id,
                object.object_id,
                object.generation,
                now_ms,
            )
            .map(|_| ())
    }
}

fn now_ms() -> Result<i64, PlatformError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock_error())
        .and_then(|duration| i64::try_from(duration.as_millis()).map_err(|_| clock_error()))
}

fn clock_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoStorageUnavailable,
        "Durable Object lifecycle clock is unavailable",
    )
}
