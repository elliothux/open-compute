//! P0.1 process-level Gate: one fresh-process scenario against the
//! real `ocd` binary, pinned stock workerd, and a local `SigV4` S3 server.

use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::{
    ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, MockS3, ObjectBackend,
    resolve_s3_credentials_with,
};
use open_compute_core::{CacheConfig, S3Config, StartupId};
use open_compute_runtime::{RuntimeLock, load_runtime_lock, recover_orphan_for_test};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const GATE_RESTART_BUDGET: usize = 2;
const PLATFORM_READY_TIMEOUT_SECS: u64 = 90;
const ADMIN_TOKEN: &str = "p0-1-admin";

struct Round {
    _dir: TempDir,
    prefix: String,
    r2_prefix: String,
    bind: String,
    data: PathBuf,
    config: PathBuf,
    key: PathBuf,
    stderr: PathBuf,
    runtime_digest: String,
    child: Option<Child>,
    tracked_pids: Vec<i32>,
    tracked_ports: Vec<u16>,
    known_tokens: Vec<String>,
    ok: bool,
}

impl Drop for Round {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id() as i32;
            for c in child_pids(pid) {
                self.tracked_pids.push(c);
            }
            kill_tree(pid);
            let _ = child.wait();
        }
        let lease = self.data.join("runtime/child.lease");
        if let Err(error) = recover_orphan_for_test(&lease, &self.runtime_digest) {
            eprintln!(
                "P0.1 Gate orphan cleanup failed with {} for {}",
                error.code(),
                lease.display()
            );
        }
        // SIGKILL leaves instance control sockets under the user runtime root;
        // InstanceControl::Drop never runs in that path.
        cleanup_instance_control_runtime();
        if !self.ok {
            retain_failure(self);
        }
    }
}

fn cleanup_instance_control_runtime() {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        let _ = fs::remove_dir_all(Path::new(&xdg).join("open-compute"));
    }
    let uid = rustix::process::getuid().as_raw();
    let _ = fs::remove_dir_all(std::env::temp_dir().join(format!("open-compute-{uid}")));
}

mod public_health_port_ignores_private_listener_that_appears_first;

#[test]
fn public_health_port_ignores_private_listener_that_appears_first() {
    public_health_port_ignores_private_listener_that_appears_first::run();
}

mod p0_1_process_gate;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_1_process_gate() {
    p0_1_process_gate::run().await;
}

mod round_drop_recovers_orphan_without_platform_handle;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_drop_recovers_orphan_without_platform_handle() {
    round_drop_recovers_orphan_without_platform_handle::run().await;
}

mod round_flow;
use round_flow::*;
mod lifecycle;
use lifecycle::*;
mod cache;
use cache::*;
mod process;
use process::*;
mod evidence;
use evidence::*;
