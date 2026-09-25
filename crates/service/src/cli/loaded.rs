//! Commands operating on one explicitly selected instance configuration.

use super::*;

fn gateway_config(
    loaded: &LoadedConfig,
    scope: ServiceScope,
    registry: Option<&InstanceRegistry>,
) -> Result<PublicGatewayConfig, PlatformError> {
    let domain = loaded.config.public_gateway.as_ref().ok_or_else(|| {
        PlatformError::new(ErrorCode::ConfigInvalid, "public gateway is not configured")
    })?;
    let shared = registry
        .ok_or_else(|| PlatformError::new(ErrorCode::ConfigInvalid, "OCD registry is unavailable"))?
        .gateway_config(scope)?
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "shared gateway settings are not configured",
            )
        })?;
    Ok(PublicGatewayConfig::resolve(domain, &shared))
}

pub(super) async fn run_loaded(
    command: Command,
    loaded: LoadedConfig,
    scope: ServiceScope,
    registry: Option<&InstanceRegistry>,
    stdout: &mut impl Write,
) -> Result<ExitCode, PlatformError> {
    match command {
        Command::Config {
            command: ConfigCommand::Check { json },
        } => {
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            write_config_check(stdout, json)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Config {
            command: ConfigCommand::GatewayDnsPlan { json },
        } => {
            let gateway = gateway_config(&loaded, scope, registry)?;
            write_gateway_dns_plan(stdout, &gateway, json)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Config {
            command: ConfigCommand::GatewayChallengeProbe { json },
        } => {
            let gateway = gateway_config(&loaded, scope, registry)?;
            crate::gateway_dns_probe::probe_public_challenge_dns(&gateway).await?;
            if json {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "schema_version": 1,
                        "command": "config_gateway_challenge_probe",
                        "result": "ok",
                    })
                )
                .map_err(|_| io_failed())?;
            } else {
                writeln!(stdout, "CHALLENGE_DNS_OK").map_err(|_| io_failed())?;
            }
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Config {
            command: ConfigCommand::GatewayDnsVerify { json, resolver },
        } => {
            let gateway = gateway_config(&loaded, scope, registry)?;
            crate::gateway_dns_verify::verify_public_gateway_dns(&gateway, &resolver).await?;
            if json {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "schema_version": 1,
                        "command": "config_gateway_dns_verify",
                        "result": "ok",
                    })
                )
                .map_err(|_| io_failed())?;
            } else {
                writeln!(stdout, "GATEWAY_DNS_OK").map_err(|_| io_failed())?;
            }
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Config {
            command: ConfigCommand::GatewayTlsProbe { json },
        } => {
            let gateway = gateway_config(&loaded, scope, registry)?;
            crate::gateway_tls::probe_worker_gateway(
                gateway.shared.https_listen,
                &gateway.base_domain,
                Duration::from_secs(10),
            )
            .await?;
            if json {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "schema_version": 1,
                        "command": "config_gateway_tls_probe",
                        "result": "ok",
                    })
                )
                .map_err(|_| io_failed())?;
            } else {
                writeln!(stdout, "GATEWAY_TLS_OK").map_err(|_| io_failed())?;
            }
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Doctor { full, json } => {
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            let mode = if full {
                DoctorMode::Full
            } else {
                DoctorMode::Basic
            };
            let gateway = loaded
                .config
                .public_gateway
                .as_ref()
                .map(|_| gateway_config(&loaded, scope, registry))
                .transpose()?;
            let report = Box::pin(doctor_report(&loaded, mode, gateway.as_ref())).await;
            report.write(stdout, json)?;
            if report.failed() {
                Ok(ExitCode::from(ExitClass::Doctor.code()))
            } else {
                Ok(ExitCode::from(ExitClass::Ok.code()))
            }
        }
        Command::Backup { command } => {
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            match command {
                BackupCommand::Create { name, json } => {
                    let result = Box::pin(interruptible_offline(Box::pin(backup_create(
                        &loaded, &name,
                    ))))
                    .await?;
                    let human = format!("SNAPSHOT_OK {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::List { json } => {
                    let result = Box::pin(backup_list(&loaded)).await?;
                    let human = format!("SNAPSHOTS_OK {}", result.len());
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::Inspect {
                    snapshot_id,
                    verify,
                    json,
                } => {
                    let result = Box::pin(backup_inspect(&loaded, &snapshot_id, verify)).await?;
                    let human = format!("SNAPSHOT_OK {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::Delete { snapshot_id, json } => {
                    let result = Box::pin(interruptible_offline(Box::pin(backup_delete(
                        &loaded,
                        &snapshot_id,
                    ))))
                    .await?;
                    let human = format!("SNAPSHOT_DELETED {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::RetentionPlan {
                    keep_last,
                    max_age_seconds,
                    keep_labels,
                    json,
                } => {
                    let result = Box::pin(backup_retention_plan(
                        &loaded,
                        keep_last,
                        max_age_seconds,
                        keep_labels,
                    ))
                    .await?;
                    let human = format!("RETENTION_PLAN_OK {}", result.delete.len());
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::CleanupIncomplete { json } => {
                    let result = Box::pin(interruptible_offline(Box::pin(
                        backup_cleanup_incomplete(&loaded),
                    )))
                    .await?;
                    let human = format!("INCOMPLETE_CLEANUP_OK {}", result.objects);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::CleanupRestore { staging_id, json } => {
                    let result = backup_cleanup_restore(&loaded, &staging_id)?;
                    let human = format!("RESTORE_STAGING_CLEANUP_OK {}", result.staging_id);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::AttestRestoreSmoke {
                    snapshot_id,
                    passed,
                    json,
                } => {
                    let result = Box::pin(interruptible_offline(Box::pin(
                        backup_attest_restore_smoke(&loaded, &snapshot_id, passed),
                    )))
                    .await?;
                    let human = format!("RESTORE_SMOKE_ATTESTED {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
                BackupCommand::Restore { snapshot_id, json } => {
                    let fallback = if registry.is_none() {
                        Some(InstanceRegistry::production()?)
                    } else {
                        None
                    };
                    let registry = registry.or(fallback.as_ref()).ok_or_else(|| {
                        PlatformError::new(
                            ErrorCode::InstanceRegistryInvalid,
                            "OCD registry is unavailable for restore",
                        )
                    })?;
                    let _scope_lock =
                        crate::run::DaemonLock::acquire_restored(registry.root_for(scope))?;
                    let other_instance_ids = registry.validate_restore_target(
                        scope,
                        &loaded.path,
                        &loaded.config.data.path,
                    )?;
                    let result = Box::pin(interruptible_offline(Box::pin(backup_restore(
                        &loaded,
                        &snapshot_id,
                        &other_instance_ids,
                    ))))
                    .await?;
                    let human = format!("RESTORE_OK {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
            }
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::SupportBundle { output, json } => {
            let registry = InstanceRegistry::production()?;
            registry.get_by_config_scope(scope, &loaded.path)?;
            let server = registry.server_config(scope)?;
            let result = Box::pin(create_support_bundle(&loaded, &output, &server)).await?;
            let human = format!("SUPPORT_BUNDLE_OK {}", result.output);
            write_result(&result, stdout, json, &human)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Scheduler {
            command: SchedulerCommand::RecoverCorrupt { backup_name },
        } => {
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            let data_dir = DataDir::acquire(&loaded.config.data)?;
            let backup = data_dir.recover_corrupt_scheduler_db(
                &backup_name,
                loaded.config.data.sqlite_busy_timeout_ms,
                open_compute_core::wall_time_ms(),
            )?;
            writeln!(stdout, "SCHEDULER_RECOVERED {}", backup.display())
                .map_err(|_| io_failed())?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Worker { .. }
        | Command::Caddy { .. }
        | Command::Licenses
        | Command::Docs { .. }
        | Command::Capabilities { .. }
        | Command::Run
        | Command::Instances { .. }
        | Command::Start
        | Command::Stop
        | Command::Restart
        | Command::Status { .. }
        | Command::Logs { .. }
        | Command::Dashboard { .. }
        | Command::Setup { .. }
        | Command::Instance { .. }
        | Command::Cache { .. }
        | Command::Upgrade { .. }
        | Command::Uninstall { .. }
        | Command::Purge { .. }
        | Command::UpdateCheck
        | Command::UpgradePreflight
        | Command::Target { .. }
        | Command::Wrangler { .. }
        | Command::Config {
            command: ConfigCommand::Init { .. },
        } => {
            unreachable!("handled before config load")
        }
    }
}
