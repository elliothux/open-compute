//! Clap derive CLI for `ocd`.

use crate::backup_cli::{
    backup_attest_restore_smoke, backup_cleanup_incomplete, backup_cleanup_restore, backup_create,
    backup_delete, backup_inspect, backup_list, backup_restore, backup_retention_plan,
    write_result,
};
use crate::capabilities::{platform_capabilities, write_capabilities};
use crate::config_load::{LoadedConfig, load_platform_config, load_platform_config_from};
use crate::doctor::{DoctorMode, doctor_report};
use crate::exit::{ExitClass, emit_failure, exit_class_for};
use crate::metrics::MetricsRegistry;
use crate::run::run_platform;
use crate::support_bundle::create_support_bundle;
use clap::{Parser, Subcommand};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::DataDir;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `ocd` command line.
#[derive(Debug, Parser)]
#[command(name = "ocd", version, about = "Open Compute daemon")]
pub struct Cli {
    /// Exact configuration path; relative values use the startup working directory.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the platform process.
    Run,
    /// Offline Worker build utilities; these do not require platform configuration.
    Worker {
        /// Worker subcommand.
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Configuration utilities.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Read-only (or explicit `--full`) environment checks.
    Doctor {
        /// Authorize object-storage canary and temporary workerd compile/start/stop.
        #[arg(long)]
        full: bool,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the versioned P1 product and release capability contract.
    Capabilities {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Offline full-platform snapshot operations.
    Backup {
        /// Backup subcommand.
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Generate a bounded, secret-scanned local support archive.
    SupportBundle {
        /// Absolute nonexistent output tar path.
        #[arg(long)]
        output: PathBuf,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Offline scheduler recovery utilities.
    Scheduler {
        /// Scheduler subcommand.
        #[command(subcommand)]
        command: SchedulerCommand,
    },
    /// Print the licenses included in this executable.
    Licenses,
    /// List or print an embedded operator runbook.
    Docs {
        /// Runbook name from the list, without the .md suffix.
        name: Option<String>,
    },
}

/// `ocd config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Write a complete starter TOML to stdout, without initializing files or secrets.
    Init {
        /// Absolute data directory to put in the generated configuration.
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Static parse and validation only.
    Check {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `ocd worker` developer-tool subcommands.
#[derive(Debug, Subcommand)]
pub enum WorkerCommand {
    /// Read versioned build JSON on stdin and write a canonical binary bundle to stdout.
    Bundle,
}

/// `ocd backup` subcommands.
#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Create and fully verify a committed offline snapshot.
    Create {
        /// Bounded human-readable audit label.
        #[arg(long)]
        name: String,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// List authenticated committed snapshots for this platform.
    List {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect one authenticated committed snapshot.
    Inspect {
        /// `UUIDv7` snapshot identity.
        #[arg(long = "snapshot")]
        snapshot_id: String,
        /// Stream and hash every owned object and immutable reference.
        #[arg(long)]
        verify: bool,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete the exact authenticated owned objects for one snapshot.
    Delete {
        /// `UUIDv7` snapshot identity.
        #[arg(long = "snapshot")]
        snapshot_id: String,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Generate an authenticated retention dry-run plan without deleting objects.
    RetentionPlan {
        /// Retain this many newest committed snapshots unconditionally.
        #[arg(long)]
        keep_last: u32,
        /// Delete only snapshots at least this old, in seconds.
        #[arg(long)]
        max_age_seconds: Option<u64>,
        /// Retain snapshots with this exact label; may be repeated.
        #[arg(long = "keep-label")]
        keep_labels: Vec<String>,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove exact-layout incomplete uploads older than the configured grace period.
    CleanupIncomplete {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove object bytes from one exact failed fresh-host restore staging identity.
    CleanupRestore {
        /// `UUIDv7` suffix reported by the retained failure receipt.
        #[arg(long = "staging")]
        staging_id: String,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record that the documented post-restore product smoke completed successfully.
    AttestRestoreSmoke {
        /// Snapshot restored by the receipt being attested.
        #[arg(long = "snapshot")]
        snapshot_id: String,
        /// Explicit operator assertion that every documented smoke step passed.
        #[arg(long)]
        passed: bool,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Restore one exact-release snapshot into a fresh data directory.
    Restore {
        /// `UUIDv7` snapshot identity.
        #[arg(long = "snapshot")]
        snapshot_id: String,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `ocd scheduler` subcommands.
#[derive(Debug, Subcommand)]
pub enum SchedulerCommand {
    /// Quarantine an uninspectable scheduler database and create an empty replacement.
    RecoverCorrupt {
        /// Unique directory name created below `data/diagnostics/scheduler-recovery/`.
        #[arg(long)]
        backup_name: String,
    },
}

/// Parse argv into [`Cli`].
pub fn parse_from<I, T>(iter: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Cli::try_parse_from(iter)
}

/// Execute a parsed CLI against stdout/stderr.
pub fn execute<'a>(
    cli: Cli,
    stdout: &'a mut impl Write,
    stderr: &'a mut impl Write,
) -> std::pin::Pin<Box<dyn Future<Output = ExitCode> + 'a>> {
    Box::pin(async move {
        let result = async {
            let startup_cwd = std::env::current_dir().map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "startup working directory is unavailable",
                )
            })?;
            Box::pin(run(cli, stdout, &startup_cwd)).await
        }
        .await;
        match result {
            Ok(code) => code,
            Err(err) => {
                let _ = emit_failure(&err, stderr);
                ExitCode::from(exit_class_for(err.code()).code())
            }
        }
    })
}

async fn run(
    cli: Cli,
    stdout: &mut impl Write,
    startup_cwd: &Path,
) -> Result<ExitCode, PlatformError> {
    if matches!(
        &cli.command,
        Command::Worker {
            command: WorkerCommand::Bundle
        }
    ) {
        crate::worker_cli::encode_bundle(std::io::stdin().lock(), stdout)?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }
    match &cli.command {
        Command::Config {
            command: ConfigCommand::Init { data_dir },
        } => {
            let data_dir = crate::config_load::lexical_absolute(startup_cwd, data_dir)?;
            crate::resources::write_config(&data_dir, stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Licenses => {
            crate::resources::write_licenses(stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Docs { name } => {
            crate::resources::write_docs(name.as_deref(), stdout)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Capabilities { json } => {
            let path = cli.config.as_deref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigPathInvalid,
                    "capabilities requires --config",
                )
            })?;
            let config = load_platform_config_from(path, startup_cwd)?.config;
            write_capabilities(&platform_capabilities(&config)?, stdout, *json)?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }
    let config_path = cli.config.as_deref().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must be provided",
        )
    })?;
    match cli.command {
        Command::Config {
            command: ConfigCommand::Check { json },
        } => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            write_config_check(stdout, json)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Doctor { full, json } => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            let mode = if full {
                DoctorMode::Full
            } else {
                DoctorMode::Basic
            };
            let report = Box::pin(doctor_report(&loaded, mode)).await;
            report.write(stdout, json)?;
            if report.failed() {
                Ok(ExitCode::from(ExitClass::Doctor.code()))
            } else {
                Ok(ExitCode::from(ExitClass::Ok.code()))
            }
        }
        Command::Backup { command } => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
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
                    let result = Box::pin(interruptible_offline(Box::pin(backup_restore(
                        &loaded,
                        &snapshot_id,
                    ))))
                    .await?;
                    let human = format!("RESTORE_OK {}", result.snapshot_id);
                    write_result(&result, stdout, json, &human)?;
                }
            }
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::SupportBundle { output, json } => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
            let result = Box::pin(create_support_bundle(&loaded, &output)).await?;
            let human = format!("SUPPORT_BUNDLE_OK {}", result.output);
            write_result(&result, stdout, json, &human)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Run => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
            Box::pin(run_platform(loaded)).await?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Scheduler {
            command: SchedulerCommand::RecoverCorrupt { backup_name },
        } => {
            let loaded = load_platform_config_from(config_path, startup_cwd)?;
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            let data_dir = DataDir::acquire(&loaded.config.data)?;
            let backup = data_dir.recover_corrupt_scheduler_db(
                &backup_name,
                loaded.config.data.sqlite_busy_timeout_ms,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                    .unwrap_or(i64::MAX),
            )?;
            writeln!(stdout, "SCHEDULER_RECOVERED {}", backup.display())
                .map_err(|_| io_failed())?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Worker { .. }
        | Command::Licenses
        | Command::Docs { .. }
        | Command::Capabilities { .. }
        | Command::Config {
            command: ConfigCommand::Init { .. },
        } => {
            unreachable!("handled before config load")
        }
    }
}

async fn interruptible_offline<T>(
    operation: impl Future<Output = Result<T, PlatformError>>,
) -> Result<T, PlatformError> {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        _ = async {
            match sigterm.as_mut() {
                Some(signal) => { signal.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => Err(offline_interrupted()),
        _ = async {
            match sigint.as_mut() {
                Some(signal) => { signal.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => Err(offline_interrupted()),
    }
}

fn offline_interrupted() -> PlatformError {
    PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "offline operation was interrupted before completion",
    )
}

fn write_config_check(out: &mut impl Write, json: bool) -> Result<(), PlatformError> {
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "command": "config_check",
                "result": "ok",
            })
        )
        .map_err(|_| io_failed())?;
    } else {
        writeln!(out, "CONFIG_OK").map_err(|_| io_failed())?;
    }
    Ok(())
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
}

/// Load helper used by tests.
pub fn load_checked(path: &Path) -> Result<LoadedConfig, PlatformError> {
    let loaded = load_platform_config(path)?;
    MetricsRegistry::validate_limits(&loaded.config.metrics)?;
    Ok(loaded)
}
