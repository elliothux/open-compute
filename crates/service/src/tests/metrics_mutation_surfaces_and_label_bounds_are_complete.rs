use super::*;

#[test]
fn metrics_mutation_surfaces_and_label_bounds_are_complete() {
    let cfg = MetricsConfig {
        max_label_value_bytes: 64,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::new(&cfg, &"v".repeat(65), "workerd")
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let reg = Arc::new(MetricsRegistry::new(&cfg, "v1", "workerd").unwrap());
    assert_eq!(
        reg.set_workerd_version(&"w".repeat(65)).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    reg.set_process_up(true);
    reg.observe_start_duration(Duration::from_millis(250));
    reg.inc_restart(RestartReason::UnexpectedExit);
    reg.inc_restart(RestartReason::ProbeFailed);
    for op in [
        SqliteOp::Open,
        SqliteOp::Migrate,
        SqliteOp::Query,
        SqliteOp::Checkpoint,
    ] {
        reg.observe_sqlite(op, Duration::from_millis(1));
    }
    for op in [
        ObjectOp::Head,
        ObjectOp::Put,
        ObjectOp::Get,
        ObjectOp::Delete,
        ObjectOp::List,
    ] {
        reg.observe_object(op, ObjectResult::Failure, Duration::from_millis(2));
        assert_eq!(reg.object_total(op, ObjectResult::Failure), 1);
    }
    for op in [
        KvOperation::Get,
        KvOperation::GetWithMetadata,
        KvOperation::GetMany,
        KvOperation::Put,
        KvOperation::Delete,
        KvOperation::List,
    ] {
        reg.observe_kv_operation(op, true, 3, 5, Duration::from_millis(4));
    }
    let successful = KvLifecycleGuard::new(reg.clone(), KvLifecycle::Backup);
    successful.success();
    drop(KvLifecycleGuard::new(reg.clone(), KvLifecycle::Restore));
    reg.inc_kv_maintenance(KvMaintenance::Gc, true);
    reg.inc_kv_maintenance(KvMaintenance::Checkpoint, false);
    reg.inc_kv_corruption(usize::MAX);
    reg.observe_kv_wal_bytes(2 * 1024 * 1024);
    reg.observe_r2_operation(R2Operation::Get, true, Duration::from_millis(6));
    reg.inc_r2_provider_error(R2Operation::Put, R2ProviderError::ResultUnknown);
    reg.inc_r2_result_unknown(false);
    reg.inc_r2_condition_failure(true);
    reg.add_r2_list_head_fanout(3);
    reg.add_r2_bytes(R2StreamDirection::Upload, 7);
    reg.add_r2_bytes(R2StreamDirection::Download, 5);
    reg.observe_d1_operation(
        D1Operation::Query,
        true,
        true,
        Duration::from_millis(3),
        2,
        0,
        17,
    );
    reg.observe_d1_queue_depth(2);
    reg.set_d1_open_databases(3);
    reg.observe_d1_wal_bytes(2 * 1024 * 1024);
    reg.inc_d1_error(D1Operation::Exec, ErrorCode::D1ResultUnknown);
    let d1_backup = D1LifecycleGuard::new(reg.clone(), D1Lifecycle::Backup);
    d1_backup.success();
    drop(D1LifecycleGuard::new(reg.clone(), D1Lifecycle::Migration));
    reg.observe_do_dispatch(DoOperation::Fetch, true, Duration::from_millis(7));
    reg.observe_do_dispatch(DoOperation::Rpc, false, Duration::from_millis(8));
    reg.observe_do_dispatch(DoOperation::Connect, true, Duration::from_millis(9));
    reg.set_do_active_hosts(4);
    for reason in [
        DoFacetReloadReason::Promotion,
        DoFacetReloadReason::Restart,
        DoFacetReloadReason::Delete,
    ] {
        reg.inc_do_facet_reload(reason);
    }
    reg.inc_do_reconcile(DoReconcileState::Creating, true);
    reg.inc_do_reconcile(DoReconcileState::Deleting, false);
    reg.set_do_storage_watermark(usize::MAX);
    reg.observe_scheduler_summary(
        SchedulerSummary {
            scheduled: 3,
            claimed: 2,
            discarding: 1,
            oldest_due_at_ms: Some(1_000),
            expired_claims: 0,
        },
        4_000,
    );
    for kind in SchedulerKind::ALL {
        reg.inc_scheduler_claim(kind, SchedulerClaimOutcome::Claimed);
        reg.inc_scheduler_claim_expired(kind, 2);
        reg.set_scheduler_in_flight(kind, 1);
    }
    for kind in SchedulerKind::ALL {
        reg.observe_scheduler_workload(
            kind,
            open_compute_core::WorkloadSummary {
                ready: 3 + kind.index() as u64,
                claimed: 2,
                expired: 0,
                oldest_due_at_ms: Some(1_000),
                next_due_at_ms: Some(1_000),
            },
            4_000,
        );
    }
    for kind in SchedulerKind::ALL {
        reg.observe_scheduler_claim_duration(kind, Duration::from_millis(4));
    }
    for (kind, state) in SchedulerKind::ALL.into_iter().zip([
        SchedulerPoolState::Ready,
        SchedulerPoolState::Paused,
        SchedulerPoolState::Backoff,
        SchedulerPoolState::CircuitOpen,
    ]) {
        reg.inc_scheduler_stale_completion(kind);
        reg.set_scheduler_pool_state(kind, state);
    }
    reg.inc_scheduler_wake("notification");
    reg.observe_alarm_delivery(AlarmOutcome::Retry, 2, Duration::from_millis(9));
    reg.inc_alarm_mutation(AlarmMutation::Set, true);
    reg.inc_alarm_repair(AlarmRepairSource::Scan, false);
    reg.observe_observability_ingest(true);
    reg.observe_observability_event(1, true);
    reg.set_observability_ingest_queue_depth(3);
    reg.set_observability_storage(42, Duration::from_secs(7));
    reg.inc_observability_truncated(true);
    reg.set_observability_tail_sessions(2);
    reg.observe_observability_tail_event(true);
    reg.inc_observability_tail_dropped(true);
    reg.observe_observability_query(false, true, Duration::from_millis(9));
    {
        let _reader = KvGaugeGuard::new(&reg, KvGauge::ReaderConnection);
        let _writer = KvGaugeGuard::new(&reg, KvGauge::WriterConnection);
        let mut staging = KvStagingGauge::new(Some(&reg));
        staging.add(7);
        let _upload = R2StreamGuard::new(&reg, R2StreamDirection::Upload);
        let _download = R2StreamGuard::new(&reg, R2StreamDirection::Download);
        reg.adjust_r2_staging_bytes(11, true);
        let active = reg.render(&PlatformStatus::starting());
        assert!(active.contains("kv_open_connections{role=\"reader\"} 1"));
        assert!(active.contains("kv_open_connections{role=\"writer\"} 1"));
        assert!(active.contains("kv_active_streams 1"));
        assert!(active.contains("kv_staging_bytes 7"));
        assert!(active.contains("r2_active_streams{direction=\"upload\"} 1"));
        assert!(active.contains("r2_active_streams{direction=\"download\"} 1"));
        assert!(active.contains("r2_staging_bytes 11"));
        reg.adjust_r2_staging_bytes(11, false);
    }
    let rendered = reg.render(&PlatformStatus::starting());
    assert!(rendered.contains("workerd_process_up 1"));
    assert!(rendered.contains(
        "kv_operations_total{operation=\"get_with_metadata\",outcome=\"success\",type=\"raw\"} 1"
    ));
    assert!(rendered.contains("kv_backup_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_restore_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("kv_gc_entries_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_checkpoint_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("kv_corruption_total{class=\"sqlite\"} 1"));
    assert!(rendered.contains("kv_open_connections{role=\"reader\"} 0"));
    assert!(rendered.contains("kv_active_streams 0"));
    assert!(rendered.contains("kv_staging_bytes 0"));
    assert!(rendered.contains("kv_wal_bytes_bucket{le=\"4194304\"} 1"));
    assert!(rendered.contains("r2_operations_total{operation=\"get\",outcome=\"success\"} 1"));
    assert!(
        rendered.contains("r2_provider_errors_total{stage=\"put\",category=\"result_unknown\"} 1")
    );
    assert!(rendered.contains("r2_result_unknown_total{operation=\"put\"} 1"));
    assert!(rendered.contains("r2_condition_failures_total{operation=\"put\"} 1"));
    assert!(rendered.contains("r2_list_head_fanout_total 3"));
    assert!(rendered.contains("r2_bytes_total{direction=\"ingress\"} 7"));
    assert!(rendered.contains("r2_bytes_total{direction=\"egress\"} 5"));
    assert!(rendered.contains("r2_active_streams{direction=\"upload\"} 0"));
    assert!(rendered.contains("r2_staging_bytes 0"));
    assert!(rendered.contains(
        "d1_operations_total{operation=\"query\",outcome=\"success\",readonly=\"true\"} 1"
    ));
    assert!(rendered.contains("d1_operation_queue_depth_bucket{le=\"4\"} 1"));
    assert!(rendered.contains("d1_open_databases 3"));
    assert!(rendered.contains("d1_wal_bytes_bucket{le=\"4194304\"} 1"));
    assert!(rendered.contains("d1_result_unknown_total{operation=\"exec\"} 1"));
    assert!(rendered.contains("d1_backup_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("d1_migration_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"fetch\",outcome=\"success\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"rpc\",outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"connect\",outcome=\"success\"} 1"));
    assert!(rendered.contains("oc_do_active_host_actors 4"));
    assert!(rendered.contains("oc_do_facet_reload_total{reason=\"promotion\"} 1"));
    assert!(
        rendered.contains("oc_do_object_reconcile_total{state=\"deleting\",outcome=\"failure\"} 1")
    );
    assert!(!rendered.contains("oc_do_websocket_active"));
    assert!(!rendered.contains("oc_do_storage_bytes"));
    assert!(rendered.contains("oc_do_storage_watermark{state=\"stop\"} 1"));
    assert!(rendered.contains("oc_do_alarm_jobs{state=\"scheduled\"} 3"));
    assert!(!rendered.contains("oc_scheduler_"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"do_alarm\"} 3"));
    assert!(
        rendered.contains(
            "open_compute_scheduler_claim_total{kind=\"do_alarm\",outcome=\"claimed\"} 1"
        )
    );
    assert!(
        rendered.contains("open_compute_scheduler_stale_completion_total{kind=\"do_alarm\"} 1")
    );
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"queue\"} 4"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"cron\"} 5"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"workflow\"} 6"));
    assert!(rendered.contains("open_compute_scheduler_in_flight{kind=\"workflow\"} 1"));
    assert!(rendered.contains("open_compute_scheduler_lease_recovery_total{kind=\"queue\"} 2"));
    assert!(
        rendered.contains("open_compute_scheduler_stale_completion_total{kind=\"workflow\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"do_alarm\",state=\"ready\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"queue\",state=\"paused\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"cron\",state=\"backoff\"} 1")
    );
    assert!(rendered.contains("open_compute_scheduler_wake_total{reason=\"notification\"} 1"));
    assert!(
        rendered.contains("oc_do_alarm_mutation_total{operation=\"set\",outcome=\"success\"} 1")
    );
    assert!(
        rendered.contains("oc_do_alarm_delivery_total{outcome=\"retry\",retry_bucket=\"2\"} 1")
    );
    assert!(rendered.contains("oc_do_alarm_repair_total{source=\"scan\",outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_alarm_lag_seconds 3"));
    assert!(rendered.contains("open_compute_observability_ingest_total{result=\"success\"} 1"));
    assert!(rendered.contains("open_compute_observability_ingest_queue_depth 3"));
    assert!(rendered.contains("open_compute_observability_db_bytes 42"));
    assert!(rendered.contains("open_compute_observability_tail_sessions 2"));
    assert!(
        rendered.contains(
            "open_compute_observability_query_total{view=\"events\",result=\"success\"} 1"
        )
    );
}
