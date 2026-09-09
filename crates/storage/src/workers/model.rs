use super::*;

pub use super::version_create::NewVersionProducts;

/// Current immutable loader descriptor schema.
pub const LOADER_SCHEMA_VERSION: i64 = 1;

/// Stable system Worker name for the operator dashboard.
pub const SYSTEM_DASHBOARD_WORKER_NAME: &str = "open-compute-dashboard";

/// Worker ownership boundary between tenant-managed and platform-owned Workers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOwnership {
    /// Tenant-managed Worker visible through the control API.
    Tenant,
    /// Platform-owned Worker excluded from tenant catalog and mutation APIs.
    System,
}

impl WorkerOwnership {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::System => "system",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "tenant" => Ok(Self::Tenant),
            "system" => Ok(Self::System),
            _ => Err(invariant()),
        }
    }
}

/// Persisted Worker lifecycle row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    /// Opaque Worker identity.
    pub id: WorkerId,
    /// Owning account.
    pub account_id: AccountId,
    /// Lowercase display slug.
    pub name: String,
    /// Current immutable traffic-assignment identity.
    pub active_deployment_id: Option<DeploymentId>,
    /// Version selected by the current Deployment, derived at read time.
    pub active_version_id: Option<VersionId>,
    /// Stable future Durable Object storage identity.
    pub do_storage_id: String,
    /// Route/promotion generation.
    pub route_generation: u64,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last mutation timestamp.
    pub updated_at_ms: i64,
    /// Tombstone timestamp.
    pub deleted_at_ms: Option<i64>,
    /// Tenant or platform ownership boundary.
    pub ownership: WorkerOwnership,
}

/// Mutable Script-level Workers Logs policy frozen into each runtime snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerObservabilitySettings {
    /// Monotonic Script setting generation.
    pub generation: u64,
    /// Master observability persistence switch.
    pub enabled: bool,
    /// Optional top-level head sampling rate.
    pub head_sampling_rate: Option<f64>,
    /// Workers Logs collection switch.
    pub logs_enabled: bool,
    /// Optional logs-specific head sampling rate.
    pub logs_head_sampling_rate: Option<f64>,
    /// Whether invocation summary events are persisted.
    pub invocation_logs: bool,
    /// Whether sampled events are persisted locally.
    pub persist: bool,
    /// Last settings mutation time.
    pub updated_at_ms: i64,
}

impl WorkerObservabilitySettings {
    /// Effective deterministic head sampling rate.
    #[must_use]
    pub fn effective_head_sampling_rate(&self) -> f64 {
        self.logs_head_sampling_rate
            .or(self.head_sampling_rate)
            .unwrap_or(1.0)
    }
}

/// Complete replacement value for one Script observability policy.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateWorkerObservabilitySettings {
    /// Master observability persistence switch.
    pub enabled: bool,
    /// Optional top-level head sampling rate.
    pub head_sampling_rate: Option<f64>,
    /// Workers Logs collection switch.
    pub logs_enabled: bool,
    /// Optional logs-specific head sampling rate.
    pub logs_head_sampling_rate: Option<f64>,
    /// Whether invocation summary events are persisted.
    pub invocation_logs: bool,
    /// Whether sampled events are persisted locally.
    pub persist: bool,
}

/// Content-free management audit for Workers Logs and realtime tail operations.
#[derive(Clone, Debug, PartialEq)]
pub enum ObservabilityAudit {
    /// One process-local Script Tail was created.
    TailCreate {
        /// Script authority.
        worker_id: WorkerId,
    },
    /// One process-local Script Tail was deleted or revoked.
    TailDelete {
        /// Script authority.
        worker_id: WorkerId,
    },
    /// One bounded telemetry query completed.
    Query {
        /// `events` or `invocations`.
        view: String,
        /// Inclusive query start in Unix milliseconds.
        from_ms: i64,
        /// Exclusive query end in Unix milliseconds.
        to_ms: i64,
        /// Public result count.
        result_count: usize,
        /// Normalized filter keys only; values are deliberately absent.
        filter_keys: Vec<String>,
    },
}

/// Immutable single-Version traffic assignment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentRecord {
    /// Opaque Deployment identity.
    pub id: DeploymentId,
    /// Parent Script/Worker.
    pub worker_id: WorkerId,
    /// Ready immutable Version receiving 100 percent of traffic.
    pub version_id: VersionId,
    /// Stable creation source.
    pub source: DeploymentSource,
    /// Closed Cloudflare deployment annotations.
    pub annotations: BTreeMap<String, String>,
    /// Creation time.
    pub created_at_ms: i64,
    /// Tombstone time for a non-current Deployment.
    pub deleted_at_ms: Option<i64>,
}

/// Stable reason a Deployment was created.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentSource {
    /// Script upload combined Version creation and activation.
    ScriptUpload,
    /// Explicit Versions/Deployments API activation.
    VersionsApi,
    /// Explicit rollback to a historical Version.
    Rollback,
    /// Platform-owned system Worker activation.
    System,
}

impl DeploymentSource {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScriptUpload => "script_upload",
            Self::VersionsApi => "versions_api",
            Self::Rollback => "rollback",
            Self::System => "system",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "script_upload" => Ok(Self::ScriptUpload),
            "versions_api" => Ok(Self::VersionsApi),
            "rollback" => Ok(Self::Rollback),
            "system" => Ok(Self::System),
            _ => Err(invariant()),
        }
    }
}

/// System-owned version slot tracked outside tenant Worker catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemOwnedVersionKind {
    /// Release-owned operator dashboard assets version.
    Dashboard,
}

impl SystemOwnedVersionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "dashboard" => Ok(Self::Dashboard),
            _ => Err(invariant()),
        }
    }
}

/// Persisted system-owned version pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemOwnedVersionRecord {
    /// Version slot identity.
    pub kind: SystemOwnedVersionKind,
    /// Owning account.
    pub account_id: AccountId,
    /// Reserved system Worker identity.
    pub worker_id: WorkerId,
    /// Current active immutable version, when installed.
    pub active_version_id: Option<VersionId>,
    /// Embedded dashboard asset tree digest pinned by this slot.
    pub assets_sha256: [u8; 32],
    /// Last pin update timestamp.
    pub updated_at_ms: i64,
}

/// Version lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionState {
    /// Metadata and env are being inserted.
    Staging,
    /// Real workerd validation is in progress.
    Validating,
    /// Immutable version may be dispatched or promoted.
    Ready,
    /// Validation deterministically failed.
    Rejected,
    /// New pins are fenced while references drain.
    Deleting,
    /// Metadata is no longer dispatchable.
    Tombstoned,
}

/// Executable or static-only content carried by an immutable version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionContentKind {
    /// Tenant Worker code, with optional static assets.
    Worker,
    /// Static assets without a fabricated tenant Worker.
    AssetsOnly,
}

impl VersionContentKind {
    /// Stable current-schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::AssetsOnly => "assets_only",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "worker" => Ok(Self::Worker),
            "assets_only" => Ok(Self::AssetsOnly),
            _ => Err(invariant()),
        }
    }
}

impl VersionState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Validating => "validating",
            Self::Ready => "ready",
            Self::Rejected => "rejected",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "staging" => Ok(Self::Staging),
            "validating" => Ok(Self::Validating),
            "ready" => Ok(Self::Ready),
            "rejected" => Ok(Self::Rejected),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Persisted immutable version metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRecord {
    /// Version identity.
    pub id: VersionId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Monotonic Worker-local version.
    pub version_number: u64,
    /// Version content union discriminator.
    pub content_kind: VersionContentKind,
    /// Lifecycle state.
    pub state: VersionState,
    /// Canonical bundle digest.
    pub artifact_sha256: Option<[u8; 32]>,
    /// Canonical bundle size.
    pub artifact_size: Option<u64>,
    /// Artifact framing schema.
    pub artifact_schema_version: Option<u32>,
    /// Main ES module.
    pub main_module: Option<String>,
    /// Hash of every runtime-effective input.
    pub worker_code_sha256: [u8; 32],
    /// Loader contract schema.
    pub loader_schema_version: u32,
    /// Immutable Worker compatibility date.
    pub compatibility_date: String,
    /// Immutable sorted Worker compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// Creation time.
    pub created_at_ms: i64,
    /// Ready time.
    pub ready_at_ms: Option<i64>,
    /// Rejection time.
    pub rejected_at_ms: Option<i64>,
    /// Stable rejection code.
    pub rejection_code: Option<String>,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

impl VersionRecord {
    /// Operator API JSON projection with hex digests.
    #[must_use]
    pub fn to_api_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "workerId": self.worker_id,
            "versionNumber": self.version_number,
            "contentKind": self.content_kind,
            "state": self.state,
            "artifactSha256": self.artifact_sha256.map(hex::encode),
            "artifactSize": self.artifact_size,
            "artifactSchemaVersion": self.artifact_schema_version,
            "mainModule": self.main_module,
            "workerCodeSha256": hex::encode(self.worker_code_sha256),
            "loaderSchemaVersion": self.loader_schema_version,
            "compatibilityDate": self.compatibility_date,
            "compatibilityFlags": self.compatibility_flags,
            "createdAtMs": self.created_at_ms,
            "readyAtMs": self.ready_at_ms,
            "rejectedAtMs": self.rejected_at_ms,
            "rejectionCode": self.rejection_code,
            "deletedAtMs": self.deleted_at_ms,
        })
    }
}

/// Secret ciphertext stored for one immutable version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredVersionSecret {
    /// Environment name.
    pub name: String,
    /// Immutable random revision.
    pub revision_id: String,
    /// AEAD envelope.
    pub envelope: SecretEnvelope,
}

/// Consistent immutable source snapshot used by `RuntimeSource`.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionSnapshot {
    /// Account identity.
    pub account_id: AccountId,
    /// Worker row.
    pub worker: WorkerRecord,
    /// Version row.
    pub version: VersionRecord,
    /// Immutable closed Cloudflare Version annotations.
    pub annotations: BTreeMap<String, String>,
    /// Static-asset authority when the version declares assets.
    pub assets: Option<crate::VersionAssetsRecord>,
    /// Canonical JSON vars keyed by env name.
    pub vars: BTreeMap<String, Vec<u8>>,
    /// Encrypted secrets keyed by env name.
    pub secrets: BTreeMap<String, StoredVersionSecret>,
    /// Immutable typed resource bindings ordered by env name.
    pub bindings: Vec<crate::VersionBindingRecord>,
    /// Immutable Queue producer bindings ordered by env name.
    pub queue_bindings: Vec<crate::QueueProducerBindingRecord>,
    /// Immutable Workflow caller bindings ordered by env name.
    pub workflow_bindings: Vec<crate::WorkflowBindingRecord>,
    /// Immutable cross-Worker Service declarations ordered by env name.
    pub services: Vec<crate::VersionServiceRecord>,
    /// Immutable default and named-entrypoint automatic-cache policies.
    pub cache_policies: Vec<crate::VersionCachePolicyRecord>,
    /// Immutable platform-provided environment bindings.
    pub builtin_bindings: Vec<crate::VersionBuiltinBindingRecord>,
}

/// Route kind supported by P0.2.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    /// Platform-owned account/worker path.
    PlatformPath,
    /// Exact canonical hostname plus path prefix.
    ExactHost,
}

impl RouteKind {
    pub(crate) fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "platform_path" => Ok(Self::PlatformPath),
            "exact_host" => Ok(Self::ExactHost),
            _ => Err(invariant()),
        }
    }
}

/// Active route metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRecord {
    /// Opaque route identity.
    pub id: String,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Route kind.
    pub kind: RouteKind,
    /// Canonical exact hostname.
    pub hostname_ascii: Option<String>,
    /// Canonical path prefix.
    pub path_prefix: String,
    /// Optional named entrypoint.
    pub entrypoint: Option<String>,
    /// Route generation at creation/update.
    pub generation: u64,
}

/// Frozen route and active version identity for one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteSnapshot {
    /// Matched route.
    pub route: RouteRecord,
    /// Matched Worker.
    pub worker: WorkerRecord,
    /// Active immutable Deployment.
    pub deployment: DeploymentRecord,
    /// Active ready version.
    pub version: VersionRecord,
    /// Static-asset authority frozen with the same active version.
    pub assets: Option<crate::VersionAssetsRecord>,
}

/// Registered reason a version must remain reachable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionReferrer {
    /// Immutable version identity.
    pub version_id: VersionId,
    /// Owning subsystem token such as `control_idempotency`.
    pub kind: String,
    /// Stable subsystem-local reference identity.
    pub ref_id: String,
    /// Registration timestamp.
    pub created_at_ms: i64,
}

/// One non-active, unreferenced version eligible for automatic retention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionCandidate {
    /// Account boundary.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Candidate version.
    pub version_id: VersionId,
}

/// Input for an immutable staging version transaction.
#[derive(Clone, Debug)]
pub struct NewVersion {
    /// Platform-generated identity.
    pub id: VersionId,
    /// Owning account.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Version content union discriminator.
    pub content_kind: VersionContentKind,
    /// Artifact digest.
    pub artifact_sha256: Option<[u8; 32]>,
    /// Artifact size.
    pub artifact_size: Option<u64>,
    /// Artifact schema.
    pub artifact_schema_version: Option<u32>,
    /// Main module.
    pub main_module: Option<String>,
    /// Descriptor digest.
    pub worker_code_sha256: [u8; 32],
    /// Immutable validated compatibility date.
    pub compatibility_date: String,
    /// Immutable validated and sorted compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// Canonical JSON vars.
    pub vars: BTreeMap<String, Vec<u8>>,
    /// Encrypted secret rows.
    pub secrets: BTreeMap<String, StoredVersionSecret>,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Transaction timestamp.
    pub now_ms: i64,
}

/// Idempotency reservation result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyReservation {
    /// Caller owns a newly inserted running row.
    Reserved,
    /// Same canonical request has already completed.
    Complete(Vec<u8>),
    /// Same canonical request is already running.
    Running,
    /// Same canonical request previously failed; value is the stable response envelope.
    Failed(Vec<u8>),
}

/// Central typed repository. The raw `SQLite` connection remains private.
#[derive(Clone, Copy, Debug)]
pub struct WorkerRepository<'a> {
    pub(super) db: &'a ControlDb,
}
