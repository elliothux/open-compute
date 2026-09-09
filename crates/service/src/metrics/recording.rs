use super::*;

/// Fixed-series metrics registry.
#[derive(Debug)]
pub struct MetricsRegistry {
    pub(super) max_label: u64,
    pub(super) inner: Mutex<Inner>,
}

impl MetricsRegistry {
    /// Construct after validating configured bounds.
    pub fn new(
        config: &MetricsConfig,
        version: &str,
        workerd_version: &str,
    ) -> Result<Self, PlatformError> {
        Self::validate_limits(config)?;
        if version.len() as u64 > config.max_label_value_bytes
            || workerd_version.len() as u64 > config.max_label_value_bytes
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics label value exceeds configured max_label_value_bytes",
            ));
        }
        Ok(Self {
            max_label: config.max_label_value_bytes,
            inner: Mutex::new(Inner {
                version: version.to_owned(),
                workerd_version: workerd_version.to_owned(),
                start_total: [0; 16],
                restart_total: [0; 4],
                process_up: 0,
                start_duration: 0.0,
                sqlite_duration: [0.0; 4],
                object_backend: ObjectStorageKind::Local,
                object_total: [0; 10],
                object_duration: [0.0; 5],
                cache_bytes: 0,
                cache_entries: 0,
                cache_hits: 0,
                integrity_errors: 0,
                resource_operations: [0; 10],
                resource_duration: [0.0; 5],
                resource_open_handles: 0,
                resource_pin_wait: 0.0,
                resource_reconcile: [0; 4],
                binding_backend_requests: [0; 6],
                binding_backend_bytes: [0; 2],
                binding_protocol_errors: 0,
                kv_operations: [0; 12],
                kv_operation_duration: [0.0; 6],
                kv_operation_bytes: [0; 12],
                kv_open_connections: [0; 2],
                kv_active_streams: 0,
                kv_staging_bytes: 0,
                kv_wal_bytes: [0; 5],
                kv_gc: [0; 2],
                kv_checkpoint: [0; 2],
                kv_backup: [0; 2],
                kv_restore: [0; 2],
                kv_corruption: [0; 3],
                r2_operations: [0; 10],
                r2_operation_duration: [0.0; 5],
                r2_bytes: [0; 2],
                r2_active_streams: [0; 2],
                r2_staging_bytes: 0,
                r2_provider_errors: [0; 15],
                r2_condition_failures: [0; 2],
                r2_list_head_fanout: 0,
                r2_result_unknown: [0; 2],
                r2_force_delete_remaining_batches: 0,
                d1_operations: [0; 12],
                d1_operation_duration: [0.0; 3],
                d1_statement_duration: [0.0; 3],
                d1_rows_output: [0; 3],
                d1_rows_written: [0; 3],
                d1_result_bytes: [0; 3],
                d1_queue_depth: [0; 5],
                d1_open_databases: 0,
                d1_wal_bytes: [0; 5],
                d1_interrupts: [0; 3],
                d1_authorizer_denials: [0; 4],
                d1_result_unknown: [0; 4],
                d1_backup: [0; 2],
                d1_restore: [0; 2],
                d1_migration: [0; 2],
                do_dispatch: [0; 6],
                do_dispatch_duration: [0.0; 3],
                do_active_hosts: 0,
                do_facet_reload: [0; 3],
                do_reconcile: [0; 4],
                do_storage_watermark: 0,
                scheduler_jobs: [0; 3],
                scheduler_claim: [0; 12],
                scheduler_dispatch_duration: [0.0; 6],
                scheduler_claim_duration: [0.0; 4],
                scheduler_oldest_due_age: [0.0; 4],
                scheduler_ready: [0; 4],
                scheduler_stale_completion: [0; 4],
                scheduler_pool_state: [0; 4],
                scheduler_wake: [0; 5],
                scheduler_claim_expired: [0; 4],
                scheduler_in_flight: [0; 4],
                alarm_mutation: [0; 6],
                alarm_delivery: [0; 42],
                alarm_repair: [0; 6],
                alarm_lag_seconds: 0.0,
                service_invocations: [0; 10],
                service_invocation_duration: [0.0; 5],
                service_roots: 0,
                service_operations: 0,
                service_retentions: 0,
                queue: queue::QueueMetrics::default(),
                workflow: workflow::WorkflowMetrics::default(),
                cache_images: cache_images::CacheImagesMetrics::default(),
                search: SearchMetrics::default(),
                observability_ingest: [0; 2],
                observability_events: [0; 6],
                observability_ingest_queue_depth: 0,
                observability_db_bytes: 0,
                observability_oldest_event_age_seconds: 0.0,
                observability_truncated: [0; 2],
                observability_tail_sessions: 0,
                observability_tail_events: [0; 2],
                observability_tail_dropped: [0; 2],
                observability_query: [0; 4],
                observability_query_duration_seconds: 0.0,
                last_supervisor: None,
                last_attempt: None,
                runtime_start: None,
                p1: P1Metrics::default(),
            }),
        })
    }

    /// Reject configured limits that cannot hold the required fixed set.
    pub fn validate_limits(config: &MetricsConfig) -> Result<(), PlatformError> {
        if config.max_series < REQUIRED_SERIES {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics.max_series cannot contain the required fixed series set",
            ));
        }
        if config.max_label_value_bytes < MIN_LABEL_VALUE_BYTES {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics.max_label_value_bytes cannot contain the required fixed labels",
            ));
        }
        Ok(())
    }

    /// Increment a start-stage counter.
    pub fn inc_start(&self, result: StartResult, stage: StartStage) {
        let i = start_index(result, stage);
        let mut g = self.lock();
        g.start_total[i] = g.start_total[i].saturating_add(1);
    }

    /// Increment a restart counter.
    pub fn inc_restart(&self, reason: RestartReason) {
        let i = restart_index(reason);
        let mut g = self.lock();
        g.restart_total[i] = g.restart_total[i].saturating_add(1);
    }

    /// Record workerd up (1) or down (0).
    pub fn set_process_up(&self, up: bool) {
        self.lock().process_up = u64::from(up);
    }

    /// Set the verified workerd version label. Must run before `/metrics` is exposed.
    pub fn set_workerd_version(&self, workerd_version: &str) -> Result<(), PlatformError> {
        if workerd_version.len() as u64 > self.max_label {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "metrics label value exceeds configured max_label_value_bytes",
            ));
        }
        self.lock().workerd_version = workerd_version.to_owned();
        Ok(())
    }

    /// Select the fixed object-backend label before the metrics listener starts.
    pub fn set_object_backend(&self, backend: ObjectStorageKind) {
        self.lock().object_backend = backend;
    }

    /// Record last start duration in seconds from supervisor timing.
    pub fn observe_start_duration(&self, duration: Duration) {
        self.lock().start_duration = duration.as_secs_f64();
    }

    /// Record the five successful preflight operations. Does nothing for a failure.
    pub fn observe_preflight_success(&self, outcome: &open_compute_artifacts::PreflightOutcome) {
        let mut g = self.lock();
        for _ in 0..outcome.puts() {
            let i = object_total_index(ObjectOp::Put, ObjectResult::Success);
            g.object_total[i] = g.object_total[i].saturating_add(1);
        }
        for _ in 0..outcome.heads() {
            let i = object_total_index(ObjectOp::Head, ObjectResult::Success);
            g.object_total[i] = g.object_total[i].saturating_add(1);
        }
        for _ in 0..outcome.gets() {
            let i = object_total_index(ObjectOp::Get, ObjectResult::Success);
            g.object_total[i] = g.object_total[i].saturating_add(1);
        }
        for _ in 0..outcome.deletes() {
            let i = object_total_index(ObjectOp::Delete, ObjectResult::Success);
            g.object_total[i] = g.object_total[i].saturating_add(1);
        }
    }

    /// Current restart counter for tests and status snapshots.
    #[must_use]
    pub fn restart_total(&self, reason: RestartReason) -> u64 {
        self.lock().restart_total[restart_index(reason)]
    }

    /// Current object-storage counter.
    #[must_use]
    pub fn object_total(&self, op: ObjectOp, result: ObjectResult) -> u64 {
        self.lock().object_total[object_total_index(op, result)]
    }

    /// Record last sqlite op duration.
    pub fn observe_sqlite(&self, op: SqliteOp, duration: Duration) {
        self.lock().sqlite_duration[sqlite_index(op)] = duration.as_secs_f64();
    }

    /// Record an object-storage request.
    pub fn observe_object(&self, op: ObjectOp, result: ObjectResult, duration: Duration) {
        let mut g = self.lock();
        g.object_total[object_total_index(op, result)] =
            g.object_total[object_total_index(op, result)].saturating_add(1);
        g.object_duration[object_op_index(op)] = duration.as_secs_f64();
    }

    /// Apply a supervisor snapshot without double-counting coalesced repeats.
    pub fn observe_supervisor(&self, snap: &SupervisorSnapshot) {
        let mut g = self.lock();
        let state = snap.state;
        g.process_up = u64::from(state == SupervisorState::Running);
        if g.last_supervisor == Some(state) && g.last_attempt == Some(snap.attempt) {
            return;
        }
        let prev_state = g.last_supervisor;
        let prev_attempt = g.last_attempt;

        if state == SupervisorState::Starting {
            g.runtime_start = Some(Instant::now());
        }
        if state == SupervisorState::Running
            && let Some(start) = g.runtime_start.take()
        {
            g.start_duration = start.elapsed().as_secs_f64();
        }

        // Attempt 1 is the initial start. Attempt N represents N-1 logical restarts,
        // including coalesced jumps (1 -> 3) and a first observation already at N.
        let accounted = prev_attempt.unwrap_or(0).saturating_sub(1);
        let observed = snap.attempt.saturating_sub(1);
        let delta = observed.saturating_sub(accounted);
        if delta > 0 {
            let i = restart_index(RestartReason::UnexpectedExit);
            g.restart_total[i] = g.restart_total[i].saturating_add(u64::from(delta));
        } else if prev_state == Some(SupervisorState::Starting)
            && state == SupervisorState::Failed
            && snap.attempt <= 1
        {
            let i = restart_index(RestartReason::ProbeFailed);
            g.restart_total[i] = g.restart_total[i].saturating_add(1);
        }

        g.last_supervisor = Some(state);
        g.last_attempt = Some(snap.attempt);
    }

    /// Cache gauges.
    pub fn set_cache(&self, bytes: u64, entries: u64, hits: u64, integrity_errors: u64) {
        let mut g = self.lock();
        g.cache_bytes = bytes;
        g.cache_entries = entries;
        g.cache_hits = hits;
        g.integrity_errors = integrity_errors;
    }

    /// Record one lifecycle operation without identifier-valued labels.
    pub fn observe_resource_operation(
        &self,
        operation: ResourceOperation,
        success: bool,
        duration: Duration,
    ) {
        let mut guard = self.lock();
        let index = operation.index();
        guard.resource_operations[index * 2 + usize::from(success)] =
            guard.resource_operations[index * 2 + usize::from(success)].saturating_add(1);
        guard.resource_duration[index] = duration.as_secs_f64();
    }

    /// Set the P0 KV handle count.
    pub fn set_resource_open_handles(&self, handles: u64) {
        self.lock().resource_open_handles = handles;
    }

    /// Record the last resource-pin drain wait.
    pub fn observe_resource_pin_wait(&self, duration: Duration) {
        self.lock().resource_pin_wait = duration.as_secs_f64();
    }

    /// Record startup reconciliation for a creating or deleting resource.
    pub fn inc_resource_reconcile(&self, deleting: bool, success: bool) {
        let index = usize::from(deleting) * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.resource_reconcile[index] = guard.resource_reconcile[index].saturating_add(1);
    }

    /// Record one authenticated binding backend request and bounded byte totals.
    pub fn observe_binding_backend(
        &self,
        operation: BindingBackendOperation,
        success: bool,
        ingress_bytes: u64,
        egress_bytes: u64,
    ) {
        let index = operation.index() * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.binding_backend_requests[index] =
            guard.binding_backend_requests[index].saturating_add(1);
        guard.binding_backend_bytes[0] =
            guard.binding_backend_bytes[0].saturating_add(ingress_bytes);
        guard.binding_backend_bytes[1] =
            guard.binding_backend_bytes[1].saturating_add(egress_bytes);
    }

    /// Count a malformed private binding frame without exposing its identifiers.
    pub fn inc_binding_protocol_error(&self) {
        let mut guard = self.lock();
        guard.binding_protocol_errors = guard.binding_protocol_errors.saturating_add(1);
    }

    /// Record one Vectorize read/query or mutation request without identity labels.
    pub(crate) fn observe_vectorize_request(&self, mutation: bool, success: bool) {
        self.lock()
            .search
            .observe_vectorize_request(mutation, success);
    }

    /// Publish one bounded Vectorize coordinator pass.
    pub(crate) fn observe_vectorize_coordinator(
        &self,
        indexes: u32,
        applied: u32,
        claimed: u32,
        blocked: u32,
    ) {
        self.lock()
            .search
            .observe_vectorize_coordinator(indexes, applied, claimed, blocked);
    }

    /// Record one AI Search operation without tenant or resource labels.
    pub(crate) fn observe_ai_search_request(&self, operation: AiSearchOperation, success: bool) {
        self.lock().search.observe_request(operation, success);
    }

    /// Publish the current durable AI Search job state counts.
    pub(crate) fn set_ai_search_jobs(&self, counts: [u64; 8]) {
        self.lock().search.set_jobs(counts);
    }

    /// Record the last bounded indexing stage duration.
    pub(crate) fn observe_ai_index_stage(&self, stage: AiIndexStage, duration: Duration) {
        self.lock().search.observe_stage(stage, duration);
    }

    /// Record one provider result using only bounded capability/outcome labels.
    pub(crate) fn observe_ai_provider(
        &self,
        capability: AiProviderCapability,
        outcome: AiProviderOutcome,
        inputs: u64,
        response_bytes: u64,
    ) {
        self.lock()
            .search
            .observe_provider(capability, outcome, inputs, response_bytes);
    }

    /// Record an AI Search immutable-object operation (`0=upload`, `1=download`, `2=gc`, `3=verify`).
    pub(crate) fn observe_ai_search_object(&self, operation: usize, success: bool) {
        self.lock().search.observe_object(operation, success);
    }

    /// Record one authenticated Service invocation without identifier-valued labels.
    pub(crate) fn observe_service_invocation(
        &self,
        operation: ServiceMetricOperation,
        success: bool,
        duration: Duration,
    ) {
        let index = operation.index();
        let mut guard = self.lock();
        guard.service_invocations[index * 2 + usize::from(success)] =
            guard.service_invocations[index * 2 + usize::from(success)].saturating_add(1);
        guard.service_invocation_duration[index] = duration.as_secs_f64();
    }

    /// Publish bounded process-local Service lifecycle gauges.
    pub(crate) fn set_service_invocation_counts(
        &self,
        roots: usize,
        operations: usize,
        retentions: usize,
    ) {
        let mut guard = self.lock();
        guard.service_roots = u64::try_from(roots).unwrap_or(u64::MAX);
        guard.service_operations = u64::try_from(operations).unwrap_or(u64::MAX);
        guard.service_retentions = u64::try_from(retentions).unwrap_or(u64::MAX);
    }

    pub(crate) fn observe_kv_operation(
        &self,
        operation: KvOperation,
        success: bool,
        ingress_bytes: u64,
        egress_bytes: u64,
        duration: Duration,
    ) {
        let index = operation.index();
        let mut guard = self.lock();
        guard.kv_operations[index * 2 + usize::from(success)] =
            guard.kv_operations[index * 2 + usize::from(success)].saturating_add(1);
        guard.kv_operation_duration[index] = duration.as_secs_f64();
        guard.kv_operation_bytes[index * 2] =
            guard.kv_operation_bytes[index * 2].saturating_add(ingress_bytes);
        guard.kv_operation_bytes[index * 2 + 1] =
            guard.kv_operation_bytes[index * 2 + 1].saturating_add(egress_bytes);
    }

    pub(crate) fn inc_kv_lifecycle(&self, lifecycle: KvLifecycle, success: bool) {
        let index = usize::from(success);
        let mut guard = self.lock();
        let values = if lifecycle.index() == 0 {
            &mut guard.kv_backup
        } else {
            &mut guard.kv_restore
        };
        values[index] = values[index].saturating_add(1);
    }

    pub(crate) fn inc_kv_maintenance(&self, maintenance: KvMaintenance, success: bool) {
        let index = usize::from(success);
        let mut guard = self.lock();
        let values = if maintenance.index() == 0 {
            &mut guard.kv_gc
        } else {
            &mut guard.kv_checkpoint
        };
        values[index] = values[index].saturating_add(1);
    }

    pub(crate) fn inc_kv_corruption(&self, class: usize) {
        let mut guard = self.lock();
        let index = class.min(guard.kv_corruption.len() - 1);
        guard.kv_corruption[index] = guard.kv_corruption[index].saturating_add(1);
    }

    /// Record one generation-authenticated observability ingest request.
    pub(crate) fn observe_observability_ingest(&self, success: bool) {
        let mut guard = self.lock();
        guard.observability_ingest[usize::from(success)] =
            guard.observability_ingest[usize::from(success)].saturating_add(1);
    }

    /// Record one canonical observability event (`0=invocation`, `1=log`, `2=exception`).
    pub(crate) fn observe_observability_event(&self, kind: usize, success: bool) {
        let index = kind.min(2) * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.observability_events[index] = guard.observability_events[index].saturating_add(1);
    }

    /// Publish the current bounded observability ingest queue depth.
    pub(crate) fn set_observability_ingest_queue_depth(&self, depth: usize) {
        self.lock().observability_ingest_queue_depth = u64::try_from(depth).unwrap_or(u64::MAX);
    }

    /// Publish independent observability storage gauges.
    pub(crate) fn set_observability_storage(&self, bytes: u64, oldest_age: Duration) {
        let mut guard = self.lock();
        guard.observability_db_bytes = bytes;
        guard.observability_oldest_event_age_seconds = oldest_age.as_secs_f64();
    }

    /// Record one collector or canonical truncation.
    pub(crate) fn inc_observability_truncated(&self, canonical: bool) {
        let mut guard = self.lock();
        guard.observability_truncated[usize::from(canonical)] =
            guard.observability_truncated[usize::from(canonical)].saturating_add(1);
    }

    /// Publish the current process-local Script Tail session count.
    pub(crate) fn set_observability_tail_sessions(&self, sessions: usize) {
        self.lock().observability_tail_sessions = u64::try_from(sessions).unwrap_or(u64::MAX);
    }

    /// Record one filtered or delivered realtime event.
    pub(crate) fn observe_observability_tail_event(&self, delivered: bool) {
        let mut guard = self.lock();
        guard.observability_tail_events[usize::from(delivered)] =
            guard.observability_tail_events[usize::from(delivered)].saturating_add(1);
    }

    /// Record one closed-client or overload realtime drop.
    pub(crate) fn inc_observability_tail_dropped(&self, overload: bool) {
        let mut guard = self.lock();
        guard.observability_tail_dropped[usize::from(overload)] =
            guard.observability_tail_dropped[usize::from(overload)].saturating_add(1);
    }

    /// Snapshot content-free realtime drop counters for operator status.
    pub(crate) fn observability_tail_dropped(&self) -> [u64; 2] {
        self.lock().observability_tail_dropped
    }

    /// Record one bounded telemetry query and its last duration.
    pub(crate) fn observe_observability_query(
        &self,
        invocations: bool,
        success: bool,
        duration: Duration,
    ) {
        let index = usize::from(invocations) * 2 + usize::from(success);
        let mut guard = self.lock();
        guard.observability_query[index] = guard.observability_query[index].saturating_add(1);
        guard.observability_query_duration_seconds = duration.as_secs_f64();
    }
}
