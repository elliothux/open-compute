use super::*;

/// Parsed command-line interface arguments.
#[derive(Debug, Parser)]
#[command(name = "ocd", version, about = "Open Compute daemon")]
pub struct Cli {
    /// Exact configuration path; relative values use the startup working directory.
    #[arg(long, global = true, conflicts_with = "instance")]
    pub config: Option<PathBuf>,
    /// Exact registered instance ID.
    #[arg(long, global = true, conflicts_with = "config")]
    pub instance: Option<InstanceSelector>,
    /// Skip upgrade reminder and asynchronous update-check refresh for this invocation.
    #[arg(long, global = true, default_value_t = false)]
    pub no_update_check: bool,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the platform process in the foreground.
    Run,
    /// Install/enable/start a managed OS service for an instance.
    Start,
    /// Stop a managed or foreground instance.
    Stop,
    /// Restart a managed instance.
    Restart,
    /// Show one instance status.
    Status {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show recent service logs.
    Logs {
        /// Follow log output when supported.
        #[arg(long)]
        follow: bool,
    },
    /// Open the operator Dashboard with a one-time login URL.
    Dashboard {
        /// Print the URL without launching a browser.
        #[arg(long)]
        no_open: bool,
        /// Emit versioned JSON (includes the short-lived URL only).
        #[arg(long)]
        json: bool,
    },
    /// Create a first-host configuration, secrets, and managed service.
    Setup {
        /// Use system-scope defaults under `/etc/open-compute`.
        #[arg(long, default_value_t = false)]
        system: bool,
        /// Apply recommended defaults without prompts.
        #[arg(long, default_value_t = false)]
        yes: bool,
    },
    /// List registered local instances.
    Instances {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Instance registration utilities.
    Instance {
        /// Instance subcommand.
        #[command(subcommand)]
        command: InstanceCommand,
    },
    /// Manage explicit remote open-compute Wrangler targets.
    Target {
        /// Target subcommand.
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Run the exact project-local Wrangler against one selected open-compute target.
    Wrangler {
        /// Explicit remote target name; mutually exclusive with --instance and --config.
        #[arg(long, conflicts_with_all = ["instance", "config"])]
        target: Option<open_compute_core::TargetName>,
        /// Project directory used as the executable resolution root and child cwd.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Wrangler command and its opaque trailing arguments.
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        arguments: Vec<OsString>,
    },
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
    /// Replace this install with a newer formal release.
    Upgrade {
        /// Exact stable `SemVer` to install; default is the latest stable release.
        version: Option<String>,
        /// Resolve and verify only; do not replace the binary.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Replace the binary without restarting managed instances.
        #[arg(long, default_value_t = false)]
        no_restart: bool,
    },
    /// Remove the receipt-owned program while preserving instance data by default.
    Uninstall {
        /// Also irreversibly delete local state for every owned instance.
        #[arg(long, default_value_t = false)]
        purge: bool,
        /// Confirm a destructive non-interactive purge.
        #[arg(long, default_value_t = false, requires = "purge")]
        yes: bool,
        /// Print the resolved operation without mutating anything.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Irreversibly delete one exact registered local instance.
    Purge {
        /// Confirm a destructive non-interactive purge.
        #[arg(long, default_value_t = false)]
        yes: bool,
        /// Print the resolved deletion plan without mutating anything.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Detached update-check helper (not for interactive use).
    #[command(name = "__update_check", hide = true)]
    UpdateCheck,
}

/// `ocd target` subcommands.
#[derive(Debug, Subcommand)]
pub enum TargetCommand {
    /// Add one explicit remote target.
    Add {
        /// Unique local target name.
        name: open_compute_core::TargetName,
        /// Remote origin followed by `/client/v4`.
        #[arg(long)]
        api_base_url: open_compute_core::TargetApiBaseUrl,
        /// Cloudflare-compatible public account ID.
        #[arg(long)]
        account_id: open_compute_core::CloudflareAccountId,
        /// Absolute owner-only deployer token file.
        #[arg(long)]
        token_file: PathBuf,
    },
    /// List registered targets without reading credentials.
    List {
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one target without reading its credential.
    Show {
        /// Exact target name.
        name: open_compute_core::TargetName,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Test authentication, account discovery, and capabilities.
    Test {
        /// Exact target name.
        name: open_compute_core::TargetName,
        /// Emit versioned JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove one target record without deleting its token file.
    Remove {
        /// Exact target name.
        name: open_compute_core::TargetName,
    },
}

/// `ocd instance` subcommands.
#[derive(Debug, Subcommand)]
pub enum InstanceCommand {
    /// Unregister a stopped service without deleting config or data.
    Unregister {
        /// Exact registered instance ID.
        #[arg(long)]
        instance: InstanceSelector,
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
