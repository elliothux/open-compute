//! Clap derive CLI for `platformd`.

use crate::config_load::{LoadedConfig, load_platform_config};
use crate::doctor::{DoctorMode, doctor_report};
use crate::exit::{ExitClass, emit_failure, exit_class_for};
use crate::metrics::MetricsRegistry;
use crate::run::run_platform;
use clap::{Parser, Subcommand};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_runtime::{PackageReleaseRequest, load_runtime_lock, package_release_bundle};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `platformd` command line.
#[derive(Debug, Parser)]
#[command(name = "platformd", version, about = "Open Compute platform daemon")]
pub struct Cli {
    /// Absolute configuration file path. Never searched from cwd or `$HOME`.
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
    /// Configuration utilities.
    Config {
        /// Config subcommand.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Read-only (or explicit `--full`) environment checks.
    Doctor {
        /// Authorize S3 canary and temporary workerd compile/start/stop.
        #[arg(long)]
        full: bool,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Fetch/verify the official pinned workerd archive and write a release layout.
    PackageRelease {
        /// Absolute destination directory. Must not already exist.
        #[arg(long)]
        dest: PathBuf,
        /// Absolute `workerd.lock.json`.
        #[arg(long)]
        lock: PathBuf,
        /// Absolute packaged runtime assets directory.
        #[arg(long)]
        assets: PathBuf,
        /// Absolute license file copied into `licenses/`.
        #[arg(long)]
        license: PathBuf,
        /// Absolute default config copied into `share/`.
        #[arg(long)]
        default_config: PathBuf,
        /// Download the official archive over HTTPS at packaging time.
        #[arg(long)]
        download: bool,
        /// Optional local official archive bytes (still hash-verified).
        #[arg(long)]
        archive: Option<PathBuf>,
    },
}

/// `platformd config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Static parse and validation only.
    Check {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
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
pub async fn execute(cli: Cli, stdout: &mut impl Write, stderr: &mut impl Write) -> ExitCode {
    execute_with_package_binary(cli, stdout, stderr, None).await
}

#[cfg(test)]
pub(crate) async fn execute_with_test_binary(
    cli: Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    platformd: &Path,
) -> ExitCode {
    execute_with_package_binary(cli, stdout, stderr, Some(platformd)).await
}

async fn execute_with_package_binary(
    cli: Cli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    platformd: Option<&Path>,
) -> ExitCode {
    match run(cli, stdout, platformd).await {
        Ok(code) => code,
        Err(err) => {
            let _ = emit_failure(&err, stderr);
            ExitCode::from(exit_class_for(err.code()).code())
        }
    }
}

async fn run(
    cli: Cli,
    stdout: &mut impl Write,
    package_binary: Option<&Path>,
) -> Result<ExitCode, PlatformError> {
    if let Command::PackageRelease {
        dest,
        lock,
        assets,
        license,
        default_config,
        download,
        archive,
    } = cli.command
    {
        if !download && archive.is_none() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "package-release requires --download or --archive",
            ));
        }
        let (parsed, _) = load_runtime_lock(&lock)?;
        let archive_bytes = match archive {
            Some(path) => Some(std::fs::read(&path).map_err(|_| {
                PlatformError::new(ErrorCode::PathInvalid, "failed to read local archive")
            })?),
            None => None,
        };
        let platformd = match package_binary {
            Some(path) => path.to_path_buf(),
            None => std::env::current_exe().map_err(|_| {
                PlatformError::new(
                    ErrorCode::PathInvalid,
                    "failed to resolve the running platformd binary",
                )
            })?,
        };
        package_release_bundle(&PackageReleaseRequest {
            lock: &parsed,
            dest_dir: &dest,
            platformd: &platformd,
            assets_dir: &assets,
            license_file: &license,
            default_config: &default_config,
            download,
            archive_bytes: archive_bytes.as_deref(),
        })?;
        writeln!(stdout, "RELEASE_OK {}", dest.display()).map_err(|_| io_failed())?;
        return Ok(ExitCode::from(ExitClass::Ok.code()));
    }
    let config_path = cli.config.as_deref().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must be absolute",
        )
    })?;
    match cli.command {
        Command::Config {
            command: ConfigCommand::Check { json },
        } => {
            let loaded = load_platform_config(config_path)?;
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            write_config_check(stdout, json)?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::Doctor { full, json } => {
            let loaded = load_platform_config(config_path)?;
            MetricsRegistry::validate_limits(&loaded.config.metrics)?;
            let mode = if full {
                DoctorMode::Full
            } else {
                DoctorMode::Basic
            };
            let report = doctor_report(&loaded, mode).await;
            report.write(stdout, json)?;
            if report.failed() {
                Ok(ExitCode::from(ExitClass::Doctor.code()))
            } else {
                Ok(ExitCode::from(ExitClass::Ok.code()))
            }
        }
        Command::Run => {
            let loaded = load_platform_config(config_path)?;
            run_platform(loaded).await?;
            Ok(ExitCode::from(ExitClass::Ok.code()))
        }
        Command::PackageRelease { .. } => unreachable!("handled before config load"),
    }
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
