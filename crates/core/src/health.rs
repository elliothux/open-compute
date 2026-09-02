//! Bounded component health and aggregate platform status.

use crate::error::ReadinessReason;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Stable component names used as bounded metrics labels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentName {
    /// `ocd` process event loop.
    Process,
    /// Data directory lock and layout.
    DataDir,
    /// Control-plane database.
    ControlDb,
    /// Master key material.
    MasterKey,
    /// Object storage / artifact authority.
    S3,
    /// Local artifact cache.
    Cache,
    /// workerd child process.
    Runtime,
    /// Independent Durable Object alarm scheduler.
    Scheduler,
    /// Snapshot and restore operator receipts.
    Operations,
    /// Per-index Vectorize SQLite authority.
    VectorizeStorage,
    /// Durable Vectorize mutation coordinator.
    VectorizeMutations,
    /// Per-instance AI Search SQLite and immutable-object authority.
    AiSearchStorage,
    /// Durable AI Search indexing coordinator.
    AiSearchIndexing,
    /// Operator-configured AI provider catalog and request pools.
    AiModels,
}

impl ComponentName {
    /// Snake-case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::DataDir => "data_dir",
            Self::ControlDb => "control_db",
            Self::MasterKey => "master_key",
            Self::S3 => "s3",
            Self::Cache => "cache",
            Self::Runtime => "runtime",
            Self::Scheduler => "scheduler",
            Self::Operations => "operations",
            Self::VectorizeStorage => "vectorize_storage",
            Self::VectorizeMutations => "vectorize_mutations",
            Self::AiSearchStorage => "ai_search_storage",
            Self::AiSearchIndexing => "ai_search_indexing",
            Self::AiModels => "ai_models",
        }
    }
}

impl Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bounded component state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    /// Component has not finished startup.
    Starting,
    /// Component is serving.
    Healthy,
    /// Component is usable with reduced capability.
    Degraded,
    /// Component failed; not ready.
    Failed,
    /// Component is shutting down.
    Draining,
}

impl ComponentState {
    /// Snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
            Self::Draining => "draining",
        }
    }

    /// Legal next states from this state.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        if self as u8 == next as u8 {
            return true;
        }
        matches!(
            (self, next),
            (
                Self::Starting,
                Self::Healthy | Self::Degraded | Self::Failed | Self::Draining
            ) | (
                Self::Healthy,
                Self::Degraded | Self::Failed | Self::Draining
            ) | (
                Self::Degraded,
                Self::Healthy | Self::Failed | Self::Draining
            ) | (Self::Failed, Self::Starting | Self::Draining)
                | (Self::Draining, Self::Failed)
        )
    }
}

/// Health snapshot for one component.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComponentHealth {
    /// Component identity.
    pub name: ComponentName,
    /// Current state.
    pub state: ComponentState,
    /// Optional stable reason.
    pub reason: Option<ReadinessReason>,
}

impl ComponentHealth {
    /// New component in `Starting`.
    #[must_use]
    pub const fn starting(name: ComponentName) -> Self {
        Self {
            name,
            state: ComponentState::Starting,
            reason: Some(ReadinessReason::Starting),
        }
    }

    /// Transition to `next` if the edge is legal.
    pub fn transition(
        &mut self,
        next: ComponentState,
        reason: Option<ReadinessReason>,
    ) -> Result<(), crate::error::PlatformError> {
        if !self.state.can_transition_to(next) {
            return Err(crate::error::PlatformError::new(
                crate::error::ErrorCode::ConfigInvalid,
                "illegal component health transition",
            ));
        }
        self.state = next;
        self.reason = reason;
        Ok(())
    }
}

/// Aggregate platform status for `/health/status`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformStatus {
    /// Overall readiness.
    pub readiness: ReadinessReason,
    /// Fixed-size component set.
    pub components: Vec<ComponentHealth>,
}

impl PlatformStatus {
    /// Status with every component starting.
    #[must_use]
    pub fn starting() -> Self {
        Self {
            readiness: ReadinessReason::Starting,
            components: vec![
                ComponentHealth::starting(ComponentName::Process),
                ComponentHealth::starting(ComponentName::DataDir),
                ComponentHealth::starting(ComponentName::ControlDb),
                ComponentHealth::starting(ComponentName::MasterKey),
                ComponentHealth::starting(ComponentName::S3),
                ComponentHealth::starting(ComponentName::Cache),
                ComponentHealth::starting(ComponentName::Runtime),
                ComponentHealth::starting(ComponentName::Scheduler),
                ComponentHealth::starting(ComponentName::Operations),
                ComponentHealth::starting(ComponentName::VectorizeStorage),
                ComponentHealth::starting(ComponentName::VectorizeMutations),
                ComponentHealth::starting(ComponentName::AiSearchStorage),
                ComponentHealth::starting(ComponentName::AiSearchIndexing),
                ComponentHealth::starting(ComponentName::AiModels),
            ],
        }
    }

    /// Recompute overall readiness from components.
    pub fn recompute(&mut self) {
        if self
            .components
            .iter()
            .any(|c| c.state == ComponentState::Draining)
        {
            self.readiness = ReadinessReason::Draining;
            return;
        }
        if let Some(failed) = self
            .components
            .iter()
            .find(|c| c.state == ComponentState::Failed)
        {
            self.readiness = failed.reason.unwrap_or(ReadinessReason::ConfigInvalid);
            return;
        }
        if self
            .components
            .iter()
            .any(|c| c.state == ComponentState::Starting)
        {
            self.readiness = ReadinessReason::Starting;
            return;
        }
        if let Some(degraded) = self
            .components
            .iter()
            .find(|c| c.state == ComponentState::Degraded)
        {
            self.readiness = degraded.reason.unwrap_or(ReadinessReason::ConfigInvalid);
            return;
        }
        self.readiness = ReadinessReason::Ready;
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
