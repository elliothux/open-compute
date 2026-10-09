"""Audited case ownership; every product-Gate case must occur exactly once.

ONCE covers fixed input/state/fault matrices, including their existing real-runtime
and restart steps. TIMING samples OS scheduling, in-flight cancellation, cleanup,
and recovery. A mixed monolithic case stays TIMING until it can be separated without
losing assertions. Discovery rejects missing, extra, and overlapping registrations.
SERVICE_PROCESS_CASES is the disjoint exclusive partition of the workspace-only
service-library inventory; it is not Cloudflare contract evidence.
"""

import json
import sys

SERVICE_PROCESS_TARGET = 'service-process-lifecycle'
SERVICE_PROCESS_CASES = (
    'doctor::tests::doctor_report_full_with_available_lock_runs_extras',
    'doctor::tests::full_runtime_checks_require_exclusive_authority_and_skip_missing_remotes',
    'tests::fail_after_stages_release_lock_and_ports::fail_after_stages_release_lock_and_ports',
    'tests::full_doctor_reports_object_storage_canary_failure_without_leaking_objects::full_doctor_reports_object_storage_canary_failure_without_leaking_objects',
    'tests::full_doctor_uses_embedded_workerd::full_doctor_uses_embedded_workerd',
    'tests::run_real_workerd_on_merged_listener_serves_status_and_shuts_down::run_real_workerd_on_merged_listener_serves_status_and_shuts_down',
    'tests::run_real_workerd_with_separate_admin_listener_and_maintenance_tick::run_real_workerd_with_separate_admin_listener_and_maintenance_tick',
    'tests::run_startup_failure_matrix_releases_owned_resources::run_startup_failure_matrix_releases_owned_resources',
)

ONCE = {
    'p3-contract': (
        'baseline-identity',
        'catalog-schema',
        'capability-catalog-bijection',
        'inventory-generation-drift',
        'inventory-member-evidence',
        'case-registry-mapping',
        'deviation-bijection',
        'compatibility-coverage',
        'public-types-surface',
        'compile-fixtures',
        'conformance-self-tests',
        'portable-fixture-inventory',
        'cloudflare-runner-safety',
    ),
    'p3-cf-diff': (
        'cache-api/portable/cache-hit',
        'd1/portable/database',
        'durable-objects/portable/object',
        'kv/portable/namespace',
        'queues/portable/producer',
        'r2/portable/bucket',
        'workers/portable/dynamic-loader',
        'workers/portable/runtime',
        'workflows/portable/lifecycle',
    ),
    # Fixed listener ordering, not a startup race.
    'p0-1': ('public_health_port_ignores_private_listener_that_appears_first',),
    # The capability case already compares two fresh CLI processes in one invocation.
    'p1-conformance': ('p1_capabilities_are_complete_and_identical_across_fresh_processes',),
    'p1-security': (
        'p1_path_corpus_and_production_fault_surface_fail_closed',
        'p1_two_instance_resource_and_version_matrix_has_no_existence_or_metric_oracle',
    ),
    # Current snapshot validation is a fixed input/fault matrix.
    'p1-snapshot': ('p1_full_snapshot_retention_and_fresh_host_restore_are_fail_closed',),
    'p3-assets': ('p3_assets_real_runtime_routing_binding_immutability_and_lifecycle',),
    'dashboard': (
        'dashboard_real_runtime_serves_spa_assets_and_cloudflare_v4_api',
    ),
    'p3-services-hard': ('p3_services_native_rpc_type_pipeline_and_lifecycle_matrix',),
    'p3-services-events': (
        'p3_service_calls_from_queue_cron_do_and_workflow_event_sources',
    ),
    # Every fault point still gets its own real child; SIGKILL follows a state marker.
    'p2-1': (
        'p2_1_fault_child',
        'p2_1_five_fresh_process_crash_boundaries_recover_exactly',
        'p2_1_schema_and_product_scope_remain_frozen',
    ),
    'p2-2': (
        'commit_crash::p2_2_queue_commit_child',
        'commit_crash::p2_2_sigkill_after_commit_preserves_message_and_counters',
    ),
    # Explicit native DO transaction rollback / Workflow mutation-rejection matrix.
    'workflow-runtime': ('output_gate::workflow_do_mutation_fails_closed_after_native_output_gate_probe',),
    # These drive explicit claims/commits/replays, not a live scheduler race.
    'workflow-recovery': (
        'snapshot_restore::workflow_snapshot_fresh_host_replays_committed_steps_with_fresh_generation',
        'transport_faults::workflow_production_step_http_known_unknown_commit_matrix',
    ),
    'runtime': (
        'invalid_compile_does_not_retry',
        'shutdown_before_start_acks_and_is_idempotent',
        'timestamps_use_deterministic_clock',
    ),
    'single-binary': (
        'readonly_commands_need_only_the_single_executable',
        'evidence::failure_evidence_copy_preserves_files_modes_and_symlinks',
    ),
    # Default Node builtins, process.env isolation, and fail-closed stubs.
    'p0-2': ('nodejs::p0_2_nodejs_default_surface_isolation_and_unsupported_stubs',),
    'p6-cf-resources': (
        'fixed_cf_resource_commands_use_live_v4_authorities',
        'w2_cf_limits_settings_clone_and_restart',
    ),
    'p6-cloudflare-sdk': ('official_cloudflare_sdk_matches_live_ocd_contract',),
    'p20-cf-cli': ('target_commands_and_cf_wrapper_preserve_the_day1_boundary',),
    'p5-ai-provider-qualification': (
        'bailian-embedding-deepseek-chat',
        'bailian-embedding-cohere-rerank-deepseek-chat',
        'bailian-embedding-bailian-rerank-deepseek-chat',
    ),
    's3-provider-qualification': ('production-preflight',),
}

TIMING = {
    'p22-browser-run': (
        'fixed_clients_actions_public_cf_and_restart_use_real_browser',
        'managed::managed_puppeteer_actions_frames_and_restart_use_real_browser',
        'managed::managed_playwright_default_page_uses_real_browser',
    ),
    'p21-python-runtime': ('runtime::p21_python_runtime_ffi_stdlib_wait_until_network_restart_rollback',),
    'p21-python-main': ('python_main::p21_python_main_upload_prepare_dispatch_restart_rollback',),
    'p21-python-services': ('services::p21_python_services_fetch_named_rpc_callback_restart_rollback',),
    'p21-python-queues': ('queues::p21_python_queues_produce_consume_retry_dlq_restart_rollback',),
    'p21-python-durable-objects': ('durable_objects::p21_python_durable_objects_fetch_rpc_storage_alarm_restart_rollback',),
    'p21-python-workflows': ('workflows::p21_python_workflows_steps_events_retry_pause_restart_rollback',),
    'p21-python-frameworks': (
        'frameworks::p21_python_django_framework_deploy_restart_rollback',
        'frameworks::p21_python_flask_framework_deploy_restart_rollback',
        'frameworks::p21_python_fastapi_framework_deploy_restart_rollback',
    ),
    'p0-1': ('p0_1_process_gate', 'round_drop_recovers_orphan_without_platform_handle'),
    # Cohesive real-runtime matrices also own concurrent requests, stream cleanup,
    # generation changes or drain assertions. Do not demote the entire matrix.
    'p0-2': (
        'p0_2_real_worker_create_validate_dispatch_promote_rollback_restart',
        'resource_limits_recovery::w2_resource_limits_protect_neighbors_and_recover_a_wedged_generation',
    ),
    'p0-3': ('p0_3_real_binding_matrix',),
    'p0-4': ('p0_4_real_kv_matrix',),
    'p0-5': (
        'p0_5_real_r2_facade_matrix',
        'uploads::concurrent_large_upload_keeps_runtime_responsive',
    ),
    'p0-6': ('p0_6_real_d1_facade_and_backend_matrix',),
    'p0-7': (
        'p0_7_real_durable_objects_matrix',
        'loader_capacity::durable_objects_preserve_loader_capacity_and_persist_through_restart',
    ),
    'p0-8': ('p0_8_real_scheduler_alarm_matrix',),
    'p0-exit': ('p0_real_combined_exit_matrix',),
    'p1-crash': ('p1_ocd_sigkill_reclaims_orphan_and_restarts_cleanly',),
    'p2-2': (
        'p2_2_real_queue_producer_matrix',
        'scheduler::p2_2_real_queue_scheduler_matrix',
    ),
    'workflow-runtime': ('workflow_runtime_suspension_timeout_parallel_and_native_errors',),
    'workflow-recovery': (
        'process_crash::workflow_ocd_sigkill_after_step_commit_replays_without_callback',
        # Also owns in-flight external-effect crashes and concurrent scheduler backlog.
        'product_bindings::workflow_step_uses_kv_d1_r2_do_queue_and_replay_preserves_external_effects',
        'transport_faults::workflow_fixture_drop_waits_for_child_reaping',
    ),
    'workflow-product': (
        'durable_batches::production_batches_enforce_join_limits_and_replay_large_outputs',
        'durable_execution::production_driver_replays_waits_retries_and_events_after_runtime_restart',
    ),
    'p2-exit': ('p2_chain_preserves_queue_handoff_frozen_workflow_and_due_work_across_sigkill',),
    'p3-services-product': (
        'p3_services_real_runtime_authority_routing_budget_and_lifecycle_matrix',
    ),
    'p3-services-recovery': (
        'p3_service_generation_exit_releases_inflight_handles_and_pins',
    ),
    # Owns background cache commits, refresh/purge fencing, and native transform admission.
    'p3-cache-images': (
        'p3_cache_images_real_runtime_semantics_and_lifecycle_matrix',
    ),
    'p6-cf-resources': (
        'worker_loader::worker_loader_native_binding_versions_delete_and_restart',
    ),
    'p5-search': ('p5_real_vectorize_ai_search_and_markdown_matrix',),
    'runtime': (
        'argv_exact_stdin_fd3_and_auth_probe',
        'begin_drain_during_startup_cancels_the_attempt',
        'begin_drain_stops_a_running_generation_gracefully',
        'compile_failure_does_not_inherit_prior_exit',
        'control_faults_reap_pid_and_pgid',
        'controlled_generation_rotation',
        'drop_does_not_signal_or_double_wait_reaped_pid',
        'drop_reaps_child',
        'ignore_term_then_kill',
        'isolated_python_preparation_owner_lifecycle',
        'isolated_python_preparation_owner_rejection_matrix',
        'late_control_event_is_unhealthy_restart',
        'lease_persist_failure_reaps_child_and_never_runs',
        'logs_bounded_and_redacted',
        'owner_registry_does_not_grow_across_restarts',
        'post_spawn_failures_reap_child',
        'reader_failure_reaches_diagnostics',
        'real_workerd_control_probe_term_kill',
        'running_resets_consecutive_backoff',
        'shutdown_cancels_slow_compile_control_probe_and_backoff',
        'shutdown_does_not_consume_budget_and_is_idempotent',
        'shutdown_waits_for_held_blocking_spawn',
        'stale_suspicion_is_dropped',
        'stalled_event_loop_restarts_once',
        'suspicion_healthy_probe_does_not_restart',
        'teardown_retains_lease_until_reap_is_proved',
        'term_and_kill_and_descendant',
        'term_grace_then_kill_order',
        'term_leader_kill_ignoring_descendant_holding_pipes',
        'unexpected_exit_backoff_and_budget',
    ),
    'single-binary': (
        'one_daemon_starts_two_isolated_instance_children',
        'single_file_first_start_restart_orphan_recovery_and_corruption_failure',
    ),
}


def validate_registry(target_names):
    """Fail closed on unaudited targets or ambiguous case ownership before building."""
    if (not SERVICE_PROCESS_CASES
            or len(SERVICE_PROCESS_CASES) != len(set(SERVICE_PROCESS_CASES))):
        raise ValueError('duplicate or empty workspace case partition')
    if SERVICE_PROCESS_TARGET in target_names:
        raise ValueError('workspace case partition collides with a product Gate target')
    if ONCE.keys() | TIMING.keys() != set(target_names):
        raise ValueError('Gate targets and case repetition registry differ')
    for name in target_names:
        cases = ONCE.get(name, ()) + TIMING.get(name, ())
        if not cases or len(cases) != len(set(cases)):
            raise ValueError(f'duplicate or empty Gate case registration: {name}')


def registered_case_ids():
    """Return the authoritative, fully-qualified native case inventory."""
    return tuple(sorted(
        f'{target}::{case}'
        for target in ONCE.keys() | TIMING.keys()
        for case in ONCE.get(target, ()) + TIMING.get(target, ())
    ))


if __name__ == '__main__':
    if sys.argv != [sys.argv[0], '--json']:
        raise SystemExit('use --json')
    print(json.dumps({'schemaVersion': 1, 'cases': registered_case_ids()}, separators=(',', ':')))
