//! Scheduler pool configuration and admission bounds.

use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};

/// One fixed scheduler workload-pool policy.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct SchedulerPoolConfig {
    /// Whether production composition enables this pool.
    pub enabled: bool,
    /// Maximum claims from this pool concurrently dispatched.
    pub max_in_flight: u32,
    /// Maximum claims selected in one short transaction.
    pub claim_batch: u32,
    /// Weighted deficit round-robin quantum.
    pub weight: u32,
}

impl SchedulerPoolConfig {
    fn alarm_default() -> Self {
        Self {
            enabled: true,
            max_in_flight: 16,
            claim_batch: 32,
            weight: 1,
        }
    }

    fn validate(self) -> bool {
        self.max_in_flight > 0
            && self.max_in_flight <= 4096
            && self.claim_batch > 0
            && self.claim_batch <= 10_000
            && self.weight > 0
            && self.weight <= 1024
    }
}

impl Default for SchedulerPoolConfig {
    fn default() -> Self {
        Self::alarm_default()
    }
}

/// Fixed pool registry for Alarm, Queue, Cron, and Workflow workloads.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "SchedulerPoolsInput")]
pub struct SchedulerPoolsConfig {
    /// Durable Object alarm pool.
    pub alarm: SchedulerPoolConfig,
    /// Queue consumer and retention-maintenance pool.
    pub queue: SchedulerPoolConfig,
    /// Cron logical-slot and dispatch pool.
    pub cron: SchedulerPoolConfig,
    /// Durable Workflow activation pool.
    pub workflow: SchedulerPoolConfig,
}

// Resolve partial TOML tables once, retaining each workload's own defaults.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct SchedulerPoolsInput {
    alarm: PoolInput,
    queue: PoolInput,
    cron: PoolInput,
    workflow: PoolInput,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct PoolInput {
    enabled: Option<bool>,
    max_in_flight: Option<u32>,
    claim_batch: Option<u32>,
    weight: Option<u32>,
}

impl PoolInput {
    fn resolve(self, defaults: SchedulerPoolConfig) -> SchedulerPoolConfig {
        SchedulerPoolConfig {
            enabled: self.enabled.unwrap_or(defaults.enabled),
            max_in_flight: self.max_in_flight.unwrap_or(defaults.max_in_flight),
            claim_batch: self.claim_batch.unwrap_or(defaults.claim_batch),
            weight: self.weight.unwrap_or(defaults.weight),
        }
    }
}

impl From<SchedulerPoolsInput> for SchedulerPoolsConfig {
    fn from(input: SchedulerPoolsInput) -> Self {
        let defaults = Self::default();
        Self {
            alarm: input.alarm.resolve(defaults.alarm),
            queue: input.queue.resolve(defaults.queue),
            cron: input.cron.resolve(defaults.cron),
            workflow: input.workflow.resolve(defaults.workflow),
        }
    }
}

impl Default for SchedulerPoolsConfig {
    fn default() -> Self {
        Self {
            alarm: SchedulerPoolConfig::alarm_default(),
            queue: SchedulerPoolConfig {
                enabled: true,
                max_in_flight: 32,
                claim_batch: 32,
                weight: 1,
            },
            cron: SchedulerPoolConfig {
                enabled: true,
                max_in_flight: 8,
                claim_batch: 8,
                weight: 1,
            },
            workflow: SchedulerPoolConfig {
                enabled: true,
                max_in_flight: 16,
                claim_batch: 16,
                weight: 1,
            },
        }
    }
}

/// Single-process multi-workload scheduler policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct SchedulerConfig {
    /// Bounded safety-reconcile interval used when no earlier wake is known.
    pub poll_interval_ms: u64,
    /// Global maximum concurrent scheduler dispatches.
    pub max_in_flight: u32,
    /// Persisted claim lease duration.
    pub claim_lease_ms: u64,
    /// Maximum time ocd waits for one workerd alarm dispatch.
    pub dispatch_timeout_ms: u64,
    /// Safety interval between dispatch timeout and claim expiry.
    pub lease_guard_ms: u64,
    /// Maximum live objects probed by one repair pass.
    pub repair_batch: u32,
    /// Delay between bounded repair passes.
    pub repair_interval_ms: u64,
    /// Maximum graceful-shutdown wait for in-flight alarm dispatches.
    pub shutdown_drain_ms: u64,
    /// Grace within which at most the newest missed Cron slot is projected.
    pub cron_misfire_grace_ms: u64,
    /// Number of retries after an initial known Cron handler failure.
    pub cron_max_retries: u8,
    /// Per-activation terminal Cron history row cap.
    pub cron_history_limit: u32,
    /// Maximum terminal Cron history age.
    pub cron_history_retention_ms: u64,
    /// Per-workload concurrency, batching, and fairness policy.
    pub pools: SchedulerPoolsConfig,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 100,
            max_in_flight: 16,
            claim_lease_ms: 60_000,
            dispatch_timeout_ms: 30_000,
            lease_guard_ms: 5_000,
            repair_batch: 100,
            repair_interval_ms: 30_000,
            shutdown_drain_ms: 10_000,
            cron_misfire_grace_ms: 300_000,
            cron_max_retries: 3,
            cron_history_limit: 100,
            cron_history_retention_ms: 7 * 24 * 60 * 60 * 1000,
            pools: SchedulerPoolsConfig::default(),
        }
    }
}

impl SchedulerConfig {
    /// Effective policy for one fixed workload kind.
    #[must_use]
    pub fn pool(&self, kind: crate::SchedulerKind) -> SchedulerPoolConfig {
        let pools = &self.pools;
        match kind {
            crate::SchedulerKind::Alarm => pools.alarm,
            crate::SchedulerKind::Queue => pools.queue,
            crate::SchedulerKind::Cron => pools.cron,
            crate::SchedulerKind::Workflow => pools.workflow,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        let guarded_timeout = self
            .dispatch_timeout_ms
            .checked_add(self.lease_guard_ms)
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::LimitInvalid, "scheduler lease bounds overflow")
            })?;
        if self.poll_interval_ms == 0
            || self.poll_interval_ms > 60_000
            || self.max_in_flight == 0
            || self.max_in_flight > 4096
            || self.dispatch_timeout_ms == 0
            || self.dispatch_timeout_ms > 5 * 60 * 1000
            || self.lease_guard_ms == 0
            || self.claim_lease_ms < guarded_timeout
            || self.claim_lease_ms > 15 * 60 * 1000
            || self.repair_batch == 0
            || self.repair_batch > 10_000
            || self.repair_interval_ms == 0
            || self.repair_interval_ms > 24 * 60 * 60 * 1000
            || self.shutdown_drain_ms > 5 * 60 * 1000
            || self.cron_misfire_grace_ms > 24 * 60 * 60 * 1000
            || self.cron_max_retries > 3
            || self.cron_history_limit == 0
            || self.cron_history_limit > 10_000
            || self.cron_history_retention_ms == 0
            || self.cron_history_retention_ms > 365 * 24 * 60 * 60 * 1000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "scheduler policy is outside the hard platform bounds",
            ));
        }
        let pools = &self.pools;
        if ![pools.alarm, pools.queue, pools.cron, pools.workflow]
            .into_iter()
            .all(SchedulerPoolConfig::validate)
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "scheduler pool policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}
