use super::*;

pub(super) fn inspect_platform(
    loaded: &LoadedConfig,
    checks: &mut Vec<DoctorCheck>,
) -> (
    Option<open_compute_storage::DataRootInspect>,
    Result<open_compute_storage::MasterKey, PlatformError>,
    Option<open_compute_storage::StableIdentity>,
) {
    checks.push(ok(
        "config",
        "static configuration parsed",
        Some("ok".into()),
    ));
    match platform_release_metadata(loaded) {
        Ok(metadata) => checks.push(ok(
            "release_identity",
            "release identity and migration registry are internally consistent",
            Some(metadata.release.platform_version),
        )),
        Err(error) => checks.push(failed(
            "release_identity",
            error.code(),
            error.message(),
            None,
        )),
    }
    if MetricsRegistry::validate_limits(&loaded.config.metrics).is_err() {
        checks[0] = failed(
            "config",
            ErrorCode::LimitInvalid,
            "metrics.max_series cannot contain the required fixed series set",
            None,
        );
    }
    match inspect_ai_provider_config(&loaded.config.ai) {
        Ok(value) => checks.push(ok(
            "ai_provider_config",
            "AI provider, model, credential, and offline tokenizer contracts are ready",
            Some(value),
        )),
        Err(error) => checks.push(failed(
            "ai_provider_config",
            error.code(),
            error.message(),
            None,
        )),
    }

    let inspect = match inspect_data_root(&loaded.config.data) {
        Ok(v) => {
            checks.push(ok(
                "data_dir",
                "data directory exists",
                Some("present".into()),
            ));
            if let Some(msg) = v.durability.doctor_warning() {
                checks.push(warning("filesystem", msg, None));
            } else {
                checks.push(ok(
                    "filesystem",
                    "filesystem durability appears local",
                    None,
                ));
            }
            match v.free_bytes {
                Some(bytes) if bytes < loaded.config.data.free_space_hard_bytes => {
                    checks.push(failed(
                        "free_space",
                        ErrorCode::DiskHardLimit,
                        "data directory free space is below the hard limit",
                        Some(bytes.to_string()),
                    ));
                }
                Some(bytes) if bytes < loaded.config.data.free_space_soft_bytes => {
                    checks.push(warning(
                        "free_space",
                        "data directory free space is below the soft limit",
                        Some(bytes.to_string()),
                    ));
                }
                Some(bytes) => checks.push(ok(
                    "free_space",
                    "data directory free space is sufficient",
                    Some(bytes.to_string()),
                )),
                None => checks.push(warning(
                    "free_space",
                    "data directory free space could not be measured",
                    None,
                )),
            }
            if v.lock_available {
                checks.push(ok("lock", "data directory lock is available", None));
            } else {
                checks.push(failed(
                    "lock",
                    ErrorCode::DataDirInUse,
                    "data directory exclusive lock is held by another instance",
                    None,
                ));
            }
            Some(v)
        }
        Err(err) => {
            checks.push(failed("data_dir", err.code(), err.message(), None));
            checks.push(skipped("filesystem", "data directory is missing"));
            checks.push(skipped("free_space", "data directory is missing"));
            checks.push(skipped("lock", "data directory is missing"));
            None
        }
    };

    let inspected_key = inspect_master_key(&loaded.config.data);

    let db_ok = match inspect.as_ref() {
        Some(root) if !root.lock_available => {
            checks.push(skipped(
                "sqlite",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "schema",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "identity",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "resource_catalog",
                "data directory exclusive lock is held by another instance",
            ));
            None
        }
        Some(root) => {
            let db_path = root.root.join("control.sqlite");
            match inspect_control_db(&db_path, loaded.config.data.sqlite_busy_timeout_ms) {
                Ok((version, identity)) => {
                    checks.push(ok(
                        "sqlite",
                        "control database quick_check passed",
                        Some(version.to_string()),
                    ));
                    checks.push(ok(
                        "schema",
                        "applied migration checksums match this binary",
                        Some(version.to_string()),
                    ));
                    if version != open_compute_storage::migrations::current_schema_version() {
                        let index = checks.len() - 1;
                        checks[index] = failed(
                            "schema",
                            ErrorCode::SchemaUnsupported,
                            "control schema does not match this implementation",
                            Some(version.to_string()),
                        );
                    }
                    let id = identity.platform_id.to_string();
                    let bounded = bound_value(&id, 36);
                    checks.push(ok(
                        "identity",
                        "stored platform identity is present",
                        Some(bounded),
                    ));
                    match inspect_resources(
                        &db_path,
                        loaded.config.data.sqlite_busy_timeout_ms,
                        1_000,
                    ) {
                        Ok(resources) if resources.is_empty() => checks.push(ok(
                            "resource_catalog",
                            "resource health catalog is empty",
                            Some("0".to_owned()),
                        )),
                        Ok(resources) => {
                            for resource in resources {
                                let code = resource.availability_code.as_deref().unwrap_or("-");
                                let value = format!(
                                    "{} {} {} {}",
                                    resource.id,
                                    resource.kind,
                                    resource.availability.as_str(),
                                    code
                                );
                                if resource.availability == ResourceAvailability::Healthy {
                                    checks.push(ok(
                                        "resource_catalog",
                                        "resource health probe is healthy",
                                        Some(bound_value(&value, 256)),
                                    ));
                                } else {
                                    checks.push(warning(
                                        "resource_catalog",
                                        "resource health probe requires attention",
                                        Some(bound_value(&value, 256)),
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            checks.push(failed(
                                "resource_catalog",
                                err.code(),
                                err.message(),
                                None,
                            ));
                        }
                    }
                    Some(identity)
                }
                Err(err) => {
                    checks.push(failed("sqlite", err.code(), err.message(), None));
                    checks.push(skipped("schema", "control database is not inspectable"));
                    checks.push(skipped("identity", "control database is not inspectable"));
                    checks.push(skipped(
                        "resource_catalog",
                        "control database is not inspectable",
                    ));
                    None
                }
            }
        }
        None => {
            checks.push(skipped("sqlite", "data directory is missing"));
            checks.push(skipped("schema", "data directory is missing"));
            checks.push(skipped("identity", "data directory is missing"));
            checks.push(skipped("resource_catalog", "data directory is missing"));
            None
        }
    };

    (inspect, inspected_key, db_ok)
}

pub(super) fn inspect_scheduler(
    loaded: &LoadedConfig,
    inspect: Option<&open_compute_storage::DataRootInspect>,
    checks: &mut Vec<DoctorCheck>,
) {
    match inspect {
        Some(root) if root.lock_available => {
            let path = root.root.join("scheduler.sqlite");
            match inspect_scheduler_db(
                &path,
                loaded.config.data.sqlite_busy_timeout_ms,
                open_compute_core::wall_time_ms(),
            ) {
                Ok(scheduler) => {
                    checks.push(workflow::inspect(loaded, &root.root));
                    let mode_ok = scheduler.journal_mode.eq_ignore_ascii_case("wal")
                        && scheduler.synchronous == 2;
                    checks.push(if mode_ok {
                        ok(
                            "scheduler_sqlite",
                            "scheduler database integrity, WAL, and FULL sync passed",
                            Some(scheduler.schema_version.to_string()),
                        )
                    } else {
                        failed(
                            "scheduler_sqlite",
                            ErrorCode::SchedulerCorrupt,
                            "scheduler database SQLite mode is invalid",
                            None,
                        )
                    });
                    checks.push(if scheduler.invalid_rows == 0 {
                        ok(
                            "scheduler_invariants",
                            "scheduler claim and token invariants passed",
                            Some("0".to_owned()),
                        )
                    } else {
                        failed(
                            "scheduler_invariants",
                            ErrorCode::SchedulerCorrupt,
                            "scheduler claim or token invariant failed",
                            Some(scheduler.invalid_rows.to_string()),
                        )
                    });
                    let summary = scheduler.summary;
                    checks.push(ok(
                        "scheduler_summary",
                        "scheduler bounded state summary inspected",
                        Some(format!(
                            "scheduled={} claimed={} discarding={} expired={}",
                            summary.scheduled,
                            summary.claimed,
                            summary.discarding,
                            summary.expired_claims
                        )),
                    ));
                    let consumers = scheduler.queue_consumers;
                    checks.push(
                        if consumers.orphan_batches == 0 && consumers.unavailable_dlq_targets == 0 {
                            ok(
                                "queue_consumer_invariants",
                                "Queue consumer batches and DLQ targets are consistent",
                                Some(format!(
                                    "consumers={} batches={} claimed={} dlq_pending={}",
                                    consumers.consumers,
                                    consumers.claimed_batches,
                                    consumers.claimed_messages,
                                    consumers.dlq_pending
                                )),
                            )
                        } else {
                            failed(
                                "queue_consumer_invariants",
                                ErrorCode::SchedulerCorrupt,
                                "Queue consumer batch or DLQ target invariant failed",
                                Some(format!(
                                    "orphan_batches={} unavailable_dlq_targets={}",
                                    consumers.orphan_batches, consumers.unavailable_dlq_targets
                                )),
                            )
                        },
                    );
                    let cron = scheduler.cron;
                    checks.push(
                        if cron.parser_version_mismatches == 0 && cron.invalid_next_fire == 0 {
                            ok(
                                "cron_invariants",
                                "Cron parser versions and next-fire projections are valid",
                                Some(format!(
                                    "schedules={} runs={} ready={} claimed={}",
                                    cron.schedules, cron.runs, cron.ready_runs, cron.claimed_runs
                                )),
                            )
                        } else {
                            failed(
                                "cron_invariants",
                                ErrorCode::SchedulerCorrupt,
                                "Cron parser version or next-fire invariant failed",
                                Some(format!(
                                    "parser_mismatch={} invalid_next_fire={}",
                                    cron.parser_version_mismatches, cron.invalid_next_fire
                                )),
                            )
                        },
                    );
                    match inspect_p23_cross_database(
                        &root.root.join("control.sqlite"),
                        &path,
                        loaded.config.data.sqlite_busy_timeout_ms,
                    ) {
                        Ok(cross)
                            if cross.queue_consumer_projection_mismatches == 0
                                && cross.cron_projection_mismatches == 0
                                && cross.version_referrer_mismatches == 0 =>
                        {
                            checks.push(ok(
                            "p2_3_cross_database",
                            "Queue/Cron projections and version referrers match control authority",
                            Some("0".to_owned()),
                        ));
                        }
                        Ok(cross) => checks.push(failed(
                            "p2_3_cross_database",
                            ErrorCode::SchedulerCorrupt,
                            "Queue/Cron projection or version-referrer authority diverged",
                            Some(format!(
                                "queue={} cron={} referrers={}",
                                cross.queue_consumer_projection_mismatches,
                                cross.cron_projection_mismatches,
                                cross.version_referrer_mismatches,
                            )),
                        )),
                        Err(error) => checks.push(failed(
                            "p2_3_cross_database",
                            error.code(),
                            error.message(),
                            None,
                        )),
                    }
                }
                Err(error) => {
                    checks.push(failed(
                        "scheduler_sqlite",
                        error.code(),
                        error.message(),
                        None,
                    ));
                    checks.push(skipped(
                        "scheduler_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "scheduler_summary",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "queue_consumer_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "cron_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "p2_3_cross_database",
                        "scheduler database is not inspectable",
                    ));
                }
            }
        }
        Some(_) => {
            checks.push(skipped(
                "scheduler_sqlite",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "scheduler_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "scheduler_summary",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "queue_consumer_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "cron_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "p2_3_cross_database",
                "data directory exclusive lock is held by another instance",
            ));
        }
        None => {
            checks.push(skipped("scheduler_sqlite", "data directory is missing"));
            checks.push(skipped("scheduler_invariants", "data directory is missing"));
            checks.push(skipped("scheduler_summary", "data directory is missing"));
            checks.push(skipped(
                "queue_consumer_invariants",
                "data directory is missing",
            ));
            checks.push(skipped("cron_invariants", "data directory is missing"));
            checks.push(skipped("p2_3_cross_database", "data directory is missing"));
        }
    }
}
