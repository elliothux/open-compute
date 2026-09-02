//! P1 immutable admission decisions and process-local byte reservations.

use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Current platform write-admission mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformMode {
    /// Normal foreground traffic is admitted subject to quotas and disk headroom.
    Serving,
    /// Terminal shutdown is draining existing work and rejects new work.
    Draining,
    /// An offline command owns the data directory.
    Offline,
}

/// Low-cardinality operation class used by admission and metrics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationClass {
    /// Worker or version control-plane mutation.
    Workers,
    /// KV mutation or staging.
    Kv,
    /// R2 mutation or staging.
    R2,
    /// D1 mutation or backup staging.
    D1,
    /// Durable Object mutation.
    DurableObjects,
    /// Scheduler authority or projection mutation.
    Scheduler,
    /// Full platform snapshot creation.
    Snapshot,
    /// Fresh-host restore staging.
    Restore,
}

/// One immutable decision input captured before a bounded mutation starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdmissionSnapshotV1 {
    /// JSON format version.
    pub schema_version: u32,
    /// Filesystem bytes available to the data directory.
    pub filesystem_free_bytes: u64,
    /// Configured soft free-space reserve.
    pub soft_reserve_bytes: u64,
    /// Configured hard free-space reserve.
    pub hard_reserve_bytes: u64,
    /// Subset of the hard reserve retained for cleanup and diagnostics.
    pub emergency_reserve_bytes: u64,
    /// Bytes reserved by in-flight bounded operations.
    pub reserved_bytes: u64,
    /// Bytes observed in project-owned staging directories.
    pub owned_staging_bytes: u64,
    /// Current lifecycle mode.
    pub mode: PlatformMode,
}

impl AdmissionSnapshotV1 {
    /// Admit an operation and compute its conservative post-reservation headroom.
    pub fn admit(self, requested_bytes: u64) -> Result<u64, PlatformError> {
        if self.schema_version != 1 || self.emergency_reserve_bytes >= self.hard_reserve_bytes {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "admission snapshot is invalid",
            ));
        }
        if self.mode != PlatformMode::Serving {
            return Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "platform is not admitting new mutations",
            ));
        }
        let committed = self
            .reserved_bytes
            .saturating_add(self.owned_staging_bytes)
            .saturating_add(requested_bytes)
            .saturating_add(self.hard_reserve_bytes);
        self.filesystem_free_bytes
            .checked_sub(committed)
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::StoragePressure,
                    "host storage reserve would be violated",
                )
            })
    }
}

/// Shared process-local reservation counter.
#[derive(Clone, Debug, Default)]
pub struct AdmissionReservations {
    bytes: Arc<AtomicU64>,
}

impl AdmissionReservations {
    /// Current in-flight reserved byte count.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Acquire)
    }

    /// Reserve a bounded amount and return an idempotent RAII release guard.
    pub fn reserve(&self, bytes: u64) -> Result<AdmissionReservation, PlatformError> {
        let previous = self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(bytes)
            })
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::AdmissionBusy,
                    "admission reservation capacity is saturated",
                )
            })?;
        let _ = previous;
        Ok(AdmissionReservation {
            reservations: self.clone(),
            bytes,
            released: false,
        })
    }
}

/// RAII byte reservation released on success, error, abort, or unwind.
#[derive(Debug)]
pub struct AdmissionReservation {
    reservations: AdmissionReservations,
    bytes: u64,
    released: bool,
}

impl AdmissionReservation {
    /// Release now. Calling this more than once has no effect.
    pub fn release(&mut self) {
        if !self.released {
            self.reservations
                .bytes
                .fetch_sub(self.bytes, Ordering::AcqRel);
            self.released = true;
        }
    }
}

impl Drop for AdmissionReservation {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
