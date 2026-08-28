//! Pinned workerd supply chain, binary verification, static config cache, and supervisor.
//!
//! The executable always contains the verified runtime payload; startup never downloads.

#![deny(missing_docs)]

pub mod compile;
mod embedded;
mod lease;
pub mod lock;
pub mod process;
pub mod supervisor;
mod verify;

mod digest;
mod fsutil;

pub use compile::{CompileRequest, CompiledConfig, PlatformReleaseMeta, compile_static_config};
pub use digest::runtime_assets_sha256;
pub use embedded::{
    RuntimePackage, embedded_payload_sha256, embedded_runtime_assets_sha256, embedded_runtime_lock,
    inspect_embedded_runtime, materialize_embedded_runtime,
};
pub use lease::assert_no_live_orphan;
#[cfg(any(test, feature = "test-support"))]
pub use lease::{recover_orphan_for_test, set_lease_write_fail, set_start_key_hook};
pub use lock::{RuntimeLock, RuntimeTarget, load_runtime_lock};
pub use process::BoundedOutput;
#[cfg(any(test, feature = "test-support"))]
pub use process::{clear_signal_log, set_reap_probe_fail, take_signal_log};
pub use supervisor::{
    ConfigCompiler, DirectoryServicePath, ExternalServiceAddress, FnCompiler,
    GenerationAuthRegistry, GenerationCredential, JitterRng, OsJitter, ProcessDiagnostics,
    READY_PATH, StaticConfigCompiler, SupervisorSnapshot, SupervisorState, TOKEN_HEADER,
    WorkerdSupervisor, WorkerdSupervisorOptions, generate_internal_token,
    probe_ready_with_raw_token, serve_argv, token_fingerprint,
};
pub use verify::VerifiedRuntime;
#[cfg(any(test, feature = "test-support"))]
pub use verify::verify_runtime_binary;

#[cfg(any(test, feature = "test-support"))]
pub use supervisor::{
    SequenceJitter, blocking_spawn_is_waiting, clear_blocking_spawn_hold, hold_blocking_spawn,
    last_spawned_pid, release_blocking_spawn, set_reader_fail_point, set_spawn_fail_point,
    take_owner_wait_count, take_reader_join_errors,
};

#[cfg(test)]
mod tests;
