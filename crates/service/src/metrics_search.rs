//! Fixed-cardinality Vectorize and AI Search product metrics.

use std::fmt::Write as _;
use std::time::Duration;

/// Coarse AI Search API operation family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiSearchOperation {
    Namespace,
    Instance,
    Item,
    Job,
    Search,
    Chat,
}

impl AiSearchOperation {
    const fn index(self) -> usize {
        self as usize
    }

    const fn token(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::Instance => "instance",
            Self::Item => "item",
            Self::Job => "job",
            Self::Search => "search",
            Self::Chat => "chat",
        }
    }
}

/// Durable indexing job state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiJobState {
    Queued,
    Claimed,
    RetryWait,
    Completed,
    Error,
    Cancelling,
    Cancelled,
    Outdated,
}

impl AiJobState {
    const fn index(self) -> usize {
        self as usize
    }

    const fn token(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::RetryWait => "retry_wait",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Outdated => "outdated",
        }
    }
}

/// Indexing pipeline stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiIndexStage {
    Parse,
    Chunk,
    Embed,
    Activate,
}

impl AiIndexStage {
    const fn index(self) -> usize {
        self as usize
    }

    const fn token(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Chunk => "chunk",
            Self::Embed => "embed",
            Self::Activate => "activate",
        }
    }
}

/// AI provider capability label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiProviderCapability {
    Embedding,
    Rewrite,
    Rerank,
    Chat,
}

impl AiProviderCapability {
    const fn index(self) -> usize {
        self as usize
    }

    const fn token(self) -> &'static str {
        match self {
            Self::Embedding => "embedding",
            Self::Rewrite => "rewrite",
            Self::Rerank => "rerank",
            Self::Chat => "chat",
        }
    }
}

/// Secret-free provider outcome class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiProviderOutcome {
    Success,
    Invalid,
    Unauthorized,
    RateLimited,
    Transient,
    Permanent,
    Timeout,
    Malformed,
}

impl AiProviderOutcome {
    const fn index(self) -> usize {
        self as usize
    }

    const fn token(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Invalid => "invalid",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Transient => "transient",
            Self::Permanent => "permanent",
            Self::Timeout => "timeout",
            Self::Malformed => "malformed",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct SearchMetrics {
    vectorize_requests: [u64; 4],
    vectorize_applied: u64,
    vectorize_indexes: u64,
    vectorize_claimed: u64,
    vectorize_blocked: u64,
    requests: [u64; 12],
    jobs: [u64; 8],
    stage_duration: [f64; 4],
    provider_requests: [u64; 32],
    provider_inputs: u64,
    provider_response_bytes: u64,
    object_operations: [u64; 8],
}

impl SearchMetrics {
    pub(super) fn observe_vectorize_request(&mut self, mutation: bool, success: bool) {
        let index = usize::from(mutation) * 2 + usize::from(success);
        self.vectorize_requests[index] = self.vectorize_requests[index].saturating_add(1);
    }

    pub(super) fn observe_vectorize_coordinator(
        &mut self,
        indexes: u32,
        applied: u32,
        claimed: u32,
        blocked: u32,
    ) {
        self.vectorize_indexes = u64::from(indexes);
        self.vectorize_applied = self.vectorize_applied.saturating_add(u64::from(applied));
        self.vectorize_claimed = u64::from(claimed);
        self.vectorize_blocked = u64::from(blocked);
    }

    pub(super) fn observe_request(&mut self, operation: AiSearchOperation, success: bool) {
        let index = operation.index() * 2 + usize::from(success);
        self.requests[index] = self.requests[index].saturating_add(1);
    }

    pub(super) fn set_jobs(&mut self, counts: [u64; 8]) {
        self.jobs = counts;
    }

    pub(super) fn observe_stage(&mut self, stage: AiIndexStage, duration: Duration) {
        self.stage_duration[stage.index()] = duration.as_secs_f64();
    }

    pub(super) fn observe_provider(
        &mut self,
        capability: AiProviderCapability,
        outcome: AiProviderOutcome,
        inputs: u64,
        response_bytes: u64,
    ) {
        let index = capability.index() * 8 + outcome.index();
        self.provider_requests[index] = self.provider_requests[index].saturating_add(1);
        self.provider_inputs = self.provider_inputs.saturating_add(inputs);
        self.provider_response_bytes = self.provider_response_bytes.saturating_add(response_bytes);
    }

    pub(super) fn observe_object(&mut self, operation: usize, success: bool) {
        let index = operation.min(3) * 2 + usize::from(success);
        self.object_operations[index] = self.object_operations[index].saturating_add(1);
    }
}

pub(super) fn write_search_metrics(
    out: &mut String,
    metrics: &SearchMetrics,
    object_backend: open_compute_core::ObjectStorageKind,
) {
    super::write_help(
        out,
        "vectorize_request_total",
        "counter",
        "Vectorize request outcomes",
    );
    for mutation in [false, true] {
        for success in [false, true] {
            let index = usize::from(mutation) * 2 + usize::from(success);
            writeln!(
                out,
                "vectorize_request_total{{operation=\"{}\",result=\"{}\"}} {}",
                if mutation { "mutation" } else { "read" },
                if success { "success" } else { "failure" },
                metrics.vectorize_requests[index]
            )
            .ok();
        }
    }
    for (name, help, value) in [
        (
            "vectorize_coordinator_applied_total",
            "Applied Vectorize mutations",
            metrics.vectorize_applied,
        ),
        (
            "vectorize_ready_indexes",
            "Ready Vectorize indexes",
            metrics.vectorize_indexes,
        ),
        (
            "vectorize_claimed_frontiers",
            "Leased Vectorize mutation frontiers",
            metrics.vectorize_claimed,
        ),
        (
            "vectorize_blocked_frontiers",
            "Permanently blocked Vectorize frontiers",
            metrics.vectorize_blocked,
        ),
    ] {
        super::write_help(
            out,
            name,
            if name.ends_with("_total") {
                "counter"
            } else {
                "gauge"
            },
            help,
        );
        writeln!(out, "{name} {value}").ok();
    }
    super::write_help(
        out,
        "ai_search_request_total",
        "counter",
        "AI Search request outcomes",
    );
    for operation in [
        AiSearchOperation::Namespace,
        AiSearchOperation::Instance,
        AiSearchOperation::Item,
        AiSearchOperation::Job,
        AiSearchOperation::Search,
        AiSearchOperation::Chat,
    ] {
        for success in [false, true] {
            let index = operation.index() * 2 + usize::from(success);
            writeln!(
                out,
                "ai_search_request_total{{operation=\"{}\",result=\"{}\"}} {}",
                operation.token(),
                if success { "success" } else { "failure" },
                metrics.requests[index]
            )
            .ok();
        }
    }
    super::write_help(
        out,
        "ai_search_jobs",
        "gauge",
        "AI Search jobs by durable state",
    );
    for state in [
        AiJobState::Queued,
        AiJobState::Claimed,
        AiJobState::RetryWait,
        AiJobState::Completed,
        AiJobState::Error,
        AiJobState::Cancelling,
        AiJobState::Cancelled,
        AiJobState::Outdated,
    ] {
        writeln!(
            out,
            "ai_search_jobs{{state=\"{}\"}} {}",
            state.token(),
            metrics.jobs[state.index()]
        )
        .ok();
    }
    super::write_help(
        out,
        "ai_search_index_stage_duration_seconds",
        "gauge",
        "Last AI Search indexing stage duration",
    );
    for stage in [
        AiIndexStage::Parse,
        AiIndexStage::Chunk,
        AiIndexStage::Embed,
        AiIndexStage::Activate,
    ] {
        writeln!(
            out,
            "ai_search_index_stage_duration_seconds{{stage=\"{}\"}} {}",
            stage.token(),
            metrics.stage_duration[stage.index()]
        )
        .ok();
    }
    super::write_help(
        out,
        "ai_provider_request_total",
        "counter",
        "AI provider request outcomes",
    );
    for capability in [
        AiProviderCapability::Embedding,
        AiProviderCapability::Rewrite,
        AiProviderCapability::Rerank,
        AiProviderCapability::Chat,
    ] {
        for outcome in [
            AiProviderOutcome::Success,
            AiProviderOutcome::Invalid,
            AiProviderOutcome::Unauthorized,
            AiProviderOutcome::RateLimited,
            AiProviderOutcome::Transient,
            AiProviderOutcome::Permanent,
            AiProviderOutcome::Timeout,
            AiProviderOutcome::Malformed,
        ] {
            writeln!(
                out,
                "ai_provider_request_total{{capability=\"{}\",outcome=\"{}\"}} {}",
                capability.token(),
                outcome.token(),
                metrics.provider_requests[capability.index() * 8 + outcome.index()]
            )
            .ok();
        }
    }
    writeln!(out, "ai_provider_inputs_total {}", metrics.provider_inputs).ok();
    writeln!(
        out,
        "ai_provider_response_bytes_total {}",
        metrics.provider_response_bytes
    )
    .ok();
    for (operation, token) in ["upload", "download", "gc", "verify"]
        .into_iter()
        .enumerate()
    {
        for success in [false, true] {
            writeln!(
                out,
                "ai_search_object_operation_total{{backend=\"{}\",operation=\"{token}\",result=\"{}\"}} {}",
                object_backend.as_str(),
                if success { "success" } else { "failure" },
                metrics.object_operations[operation * 2 + usize::from(success)]
            )
            .ok();
        }
    }
}
