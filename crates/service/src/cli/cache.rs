//! Scoped cache-clean command. The daemon owns online cleanup; offline work holds its locks.

use super::*;
use crate::instance_ops::read_daemon;
use crate::run::daemon_control::{ControlRequest, exchange};
use crate::run::{DaemonLock, OfflineInstanceOwner, clean_global_cache};
use open_compute_artifacts::CacheCleanReport;

pub(super) async fn run(
    cli: &Cli,
    command: &CacheCommand,
    deps: &OperatorDeps,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    if cli.config.is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "cache clean selects the OCD scope or a registered --instance",
        ));
    }
    let scope = if cli.system {
        ServiceScope::System
    } else {
        ServiceScope::User
    };
    let root = deps.registry.root_for(scope);
    let CacheCommand::Clean { all, dry_run } = command;
    if *all && cli.instance.is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "--all and --instance are mutually exclusive",
        ));
    }
    if read_daemon(root)?.is_some() {
        run_online(cli, *all, *dry_run, &deps.registry, scope, out)
    } else {
        let _scope_lock = DaemonLock::acquire_existing(root)?;
        run_offline(cli, *all, *dry_run, &deps.registry, scope, out).await
    }
}

fn run_online(
    cli: &Cli,
    all: bool,
    dry_run: bool,
    registry: &InstanceRegistry,
    scope: ServiceScope,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let root = registry.root_for(scope);
    if let Some(selector) = &cli.instance {
        let record = registry.get_scope(scope, selector)?;
        return single(
            out,
            &record.instance_id,
            request(
                root,
                &ControlRequest::CleanCache {
                    instance_id: record.instance_id()?,
                    dry_run,
                },
            ),
            dry_run,
        );
    }
    let mut failed = false;
    let global = request(root, &ControlRequest::CleanGlobalCache { dry_run });
    if all {
        emit(out, "global", global, dry_run, &mut failed)?;
        for record in registry.list_scope(scope)? {
            emit(
                out,
                &record.instance_id,
                request(
                    root,
                    &ControlRequest::CleanCache {
                        instance_id: record.instance_id()?,
                        dry_run,
                    },
                ),
                dry_run,
                &mut failed,
            )?;
        }
    } else {
        return single(out, "global", global, dry_run);
    }
    finish(failed)
}

async fn run_offline(
    cli: &Cli,
    all: bool,
    dry_run: bool,
    registry: &InstanceRegistry,
    scope: ServiceScope,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let root = registry.root_for(scope);
    if let Some(selector) = &cli.instance {
        let record = registry.get_scope(scope, selector)?;
        let owner = OfflineInstanceOwner::acquire(root, &record, dry_run)?;
        return single(
            out,
            &record.instance_id,
            owner.clean(dry_run).await,
            dry_run,
        );
    }
    let records = registry.list_scope(scope)?;
    if !all {
        let _owners = records
            .iter()
            .map(|record| OfflineInstanceOwner::acquire(root, record, dry_run))
            .collect::<Result<Vec<_>, _>>()?;
        recover_gateway(root, dry_run)?;
        return single(
            out,
            "global",
            clean_global_cache(root, dry_run, false),
            dry_run,
        );
    }
    let mut failed = false;
    let mut owners = Vec::new();
    for record in records {
        match OfflineInstanceOwner::acquire(root, &record, dry_run) {
            Ok(owner) => owners.push((record, owner)),
            Err(error) => emit(out, &record.instance_id, Err(error), dry_run, &mut failed)?,
        }
    }
    let global = if failed {
        Err(PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "global cache requires every registered instance to be verified",
        ))
    } else {
        recover_gateway(root, dry_run).and_then(|()| clean_global_cache(root, dry_run, false))
    };
    emit(out, "global", global, dry_run, &mut failed)?;
    for (record, owner) in owners {
        emit(
            out,
            &record.instance_id,
            owner.clean(dry_run).await,
            dry_run,
            &mut failed,
        )?;
    }
    finish(failed)
}

fn recover_gateway(root: &Path, dry_run: bool) -> Result<(), PlatformError> {
    let gateway = root.join("gateway");
    let run = gateway.join("run");
    for directory in [&gateway, &run] {
        match std::fs::symlink_metadata(directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "shared Gateway directory is inaccessible",
                ));
            }
            Ok(_) => {
                let _ = open_compute_runtime::open_host_directory_nofollow(directory)?;
            }
        }
    }
    let lease = run.join("caddy.lease");
    if dry_run {
        open_compute_runtime::assert_no_live_orphan(
            &lease,
            open_compute_runtime::embedded_caddy_sha256(),
        )
    } else {
        open_compute_runtime::PersistentHostProcess::recover_recorded_orphan(&lease)
    }
}

fn request(root: &Path, command: &ControlRequest) -> Result<CacheCleanReport, PlatformError> {
    let response = exchange(root, command)?;
    if !response.ok {
        return Err(PlatformError::new(
            response
                .error
                .as_deref()
                .and_then(ErrorCode::from_stable_str)
                .unwrap_or(ErrorCode::RuntimeUnavailable),
            "daemon rejected cache cleanup",
        ));
    }
    response.cache_report.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "daemon omitted cache cleanup report",
        )
    })
}

fn emit(
    out: &mut impl Write,
    target: &str,
    result: Result<CacheCleanReport, PlatformError>,
    dry_run: bool,
    failed: &mut bool,
) -> Result<(), PlatformError> {
    match result {
        Ok(report) => {
            *failed |= report.failed > 0;
            write_report(out, target, &report, dry_run)
        }
        Err(error) => {
            *failed = true;
            writeln!(
                out,
                "CACHE_CLEAN_FAILED target={target} code={}",
                error.code()
            )
            .map_err(|_| io_failed())
        }
    }
}

fn single(
    out: &mut impl Write,
    target: &str,
    result: Result<CacheCleanReport, PlatformError>,
    dry_run: bool,
) -> Result<(), PlatformError> {
    let mut failed = false;
    emit(out, target, result, dry_run, &mut failed)?;
    finish(failed)
}

fn write_report(
    out: &mut impl Write,
    target: &str,
    report: &CacheCleanReport,
    dry_run: bool,
) -> Result<(), PlatformError> {
    writeln!(
        out,
        "CACHE_CLEAN target={target} dry_run={dry_run} bytes={} entries={} skipped={} failed={} reason={}",
        report.bytes, report.entries, report.skipped, report.failed,
        report.failure_reason.as_deref().unwrap_or("-"),
    )
    .map_err(|_| io_failed())
}

fn finish(failed: bool) -> Result<(), PlatformError> {
    if failed {
        Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "one or more cache cleanup targets failed",
        ))
    } else {
        Ok(())
    }
}
