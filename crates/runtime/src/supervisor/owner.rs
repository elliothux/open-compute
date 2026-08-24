//! Single-owner process-group lease for a spawned runtime child.

use crate::process::{process_group_live, terminate_group_kill, terminate_group_term, wait_reaped};
use open_compute_core::{ErrorCode, PlatformError};
use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use super::logs::{LogCollector, LogTail, read_pipe_into};

struct PendingChild {
    child: Option<Child>,
    pid: i32,
}

impl Drop for PendingChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_group_kill(Some(self.pid));
            let _ = child.wait();
        }
    }
}

#[derive(Clone)]
struct OwnerEntry {
    id: u64,
    tx: mpsc::Sender<OwnerCmd>,
}

/// Cloned into the public supervisor so Drop can command owners without a snapshot PID.
#[derive(Clone, Default)]
pub(crate) struct OwnerRegistry {
    next_id: Arc<AtomicU64>,
    senders: Arc<std::sync::Mutex<Vec<OwnerEntry>>>,
}

impl OwnerRegistry {
    pub(crate) fn register(&self, tx: mpsc::Sender<OwnerCmd>) -> u64 {
        let id = self
            .next_id
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let mut guard = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.retain(|entry| entry.tx.send(OwnerCmd::Ping).is_ok());
        guard.push(OwnerEntry { id, tx });
        id
    }

    pub(crate) fn unregister(&self, id: u64) {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| entry.id != id);
    }

    pub(crate) fn kill_all(&self) {
        let senders: Vec<_> = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        for entry in senders {
            let _ = entry.tx.send(OwnerCmd::Shutdown {
                grace: Duration::from_millis(0),
                kill_after: Duration::from_secs(2),
                ack: None,
            });
        }
    }

    /// Active owner registrations after pruning disconnected senders.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn active_count(&self) -> usize {
        let mut guard = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.retain(|entry| entry.tx.send(OwnerCmd::Ping).is_ok());
        guard.len()
    }
}

pub(crate) enum OwnerCmd {
    Ping,
    Shutdown {
        grace: Duration,
        kill_after: Duration,
        ack: Option<oneshot::Sender<OwnerCompletion>>,
    },
}

/// Outcome of the unique owner thread after the child is reaped.
#[derive(Clone, Debug)]
pub(crate) struct OwnerCompletion {
    pub status: Option<ExitStatus>,
    pub stdout: LogTail,
    pub stderr: LogTail,
    pub reader_failed: bool,
}

impl OwnerCompletion {
    pub(crate) fn exit_code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }

    pub(crate) fn signal(&self) -> Option<i32> {
        self.status.and_then(|s| s.signal())
    }
}

/// Handle to the unique owner thread that may signal and wait the child.
pub(crate) struct ChildHandle {
    pub pid: i32,
    pub pgid: i32,
    cmd_tx: mpsc::Sender<OwnerCmd>,
    owner: Option<JoinHandle<()>>,
    leader_alive: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
    completion: Arc<std::sync::Mutex<Option<OwnerCompletion>>>,
}

impl ChildHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        child: Child,
        pid: i32,
        pgid: i32,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
        stdout_log: LogCollector,
        stderr_log: LogCollector,
        registry: &OwnerRegistry,
    ) -> Result<Self, PlatformError> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let leader_alive = Arc::new(AtomicBool::new(true));
        let reaped = Arc::new(AtomicBool::new(false));
        let completion = Arc::new(std::sync::Mutex::new(None));
        let alive = leader_alive.clone();
        let reaped_flag = reaped.clone();
        let completion_slot = completion.clone();
        let registry_for_owner = registry.clone();
        let registry_id = registry.register(cmd_tx.clone());
        let builder = std::thread::Builder::new().name("oc-rt-owner".into());
        let mut pending = PendingChild {
            child: Some(child),
            pid,
        };
        match builder.spawn(move || {
            let child = pending.child.take().expect("pending child");
            owner_loop(OwnerState {
                child,
                pid,
                pgid,
                stdout,
                stderr,
                stdout_log,
                stderr_log,
                cmd_rx,
                leader_alive: alive,
                reaped: reaped_flag,
                completion: completion_slot,
                registry: registry_for_owner,
                registry_id,
            });
        }) {
            Ok(owner) => Ok(Self {
                pid,
                pgid,
                cmd_tx,
                owner: Some(owner),
                leader_alive,
                reaped,
                completion,
            }),
            Err(_) => {
                registry.unregister(registry_id);
                Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to start runtime process owner thread",
                ))
            }
        }
    }

    pub(crate) fn leader_alive(&self) -> bool {
        self.leader_alive.load(Ordering::SeqCst)
    }

    pub(crate) async fn shutdown(
        mut self,
        grace: Duration,
        kill_after: Duration,
    ) -> OwnerCompletion {
        if self.finish_if_reaped().await {
            return self.take_completion();
        }
        let (ack, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(OwnerCmd::Shutdown {
                grace,
                kill_after,
                ack: Some(ack),
            })
            .is_err()
        {
            self.join_owner().await;
            return self.take_completion();
        }
        let from_ack = rx.await.ok();
        self.join_owner().await;
        from_ack.unwrap_or_else(|| self.take_completion())
    }

    pub(crate) fn shutdown_blocking(
        mut self,
        grace: Duration,
        kill_after: Duration,
    ) -> OwnerCompletion {
        let _ = self.cmd_tx.send(OwnerCmd::Shutdown {
            grace,
            kill_after,
            ack: None,
        });
        if let Some(handle) = self.owner.take() {
            let _ = handle.join();
        }
        self.take_completion()
    }

    fn take_completion(&self) -> OwnerCompletion {
        self.completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or(OwnerCompletion {
                status: None,
                stdout: LogTail::default(),
                stderr: LogTail::default(),
                reader_failed: false,
            })
    }

    async fn finish_if_reaped(&mut self) -> bool {
        if !self.reaped.load(Ordering::SeqCst) {
            return false;
        }
        self.join_owner().await;
        true
    }

    async fn join_owner(&mut self) {
        if let Some(handle) = self.owner.take() {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = handle.join();
            })
            .await;
        }
    }
}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(OwnerCmd::Shutdown {
            grace: Duration::from_millis(0),
            kill_after: Duration::from_secs(2),
            ack: None,
        });
        // Detach the owner thread so Drop never blocks a Tokio worker and
        // never aborts the only cleanup owner.
        let _ = self.owner.take();
    }
}

struct OwnerState {
    child: Child,
    pid: i32,
    pgid: i32,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    stdout_log: LogCollector,
    stderr_log: LogCollector,
    cmd_rx: mpsc::Receiver<OwnerCmd>,
    leader_alive: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
    completion: Arc<std::sync::Mutex<Option<OwnerCompletion>>>,
    registry: OwnerRegistry,
    registry_id: u64,
}

fn owner_loop(mut state: OwnerState) {
    let stdout_thread = state.stdout.take().map(|pipe| {
        let collector = state.stdout_log.clone();
        std::thread::spawn(move || read_pipe_into(pipe, &collector))
    });
    let stderr_thread = state.stderr.take().map(|pipe| {
        let collector = state.stderr_log.clone();
        std::thread::spawn(move || read_pipe_into(pipe, &collector))
    });

    let mut status: Option<ExitStatus> = None;
    let mut done_ack: Option<oneshot::Sender<OwnerCompletion>> = None;
    let mut shutting_down = false;
    let mut sent_term = false;
    let mut sent_kill = false;
    let mut term_at: Option<Instant> = None;
    let mut kill_at: Option<Instant> = None;
    let mut grace = Duration::from_millis(0);
    let mut kill_after = Duration::from_secs(2);

    loop {
        if !shutting_down {
            match state.cmd_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(OwnerCmd::Shutdown {
                    grace: g,
                    kill_after: k,
                    ack,
                }) => {
                    shutting_down = true;
                    grace = g;
                    kill_after = k;
                    done_ack = ack;
                }
                Ok(OwnerCmd::Ping) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    shutting_down = true;
                    grace = Duration::from_millis(0);
                    kill_after = Duration::from_secs(2);
                }
            }
        } else {
            while let Ok(cmd) = state.cmd_rx.try_recv() {
                if let OwnerCmd::Shutdown { ack, .. } = cmd
                    && let Some(ack) = ack
                {
                    done_ack = Some(ack);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let leader_live = !leader_has_exited(state.pid);
        if !leader_live {
            state.leader_alive.store(false, Ordering::SeqCst);
        }

        if shutting_down {
            let now = Instant::now();
            let group_live = process_group_live(state.pgid);
            if status.is_none() {
                if !sent_term {
                    terminate_group_term(Some(state.pgid));
                    sent_term = true;
                    term_at = Some(now);
                } else if !sent_kill {
                    let grace_done =
                        term_at.is_some_and(|t| now.saturating_duration_since(t) >= grace);
                    if grace_done && (leader_is_live(state.pid) || group_live) {
                        terminate_group_kill(Some(state.pgid));
                        sent_kill = true;
                        kill_at = Some(now);
                    }
                }
            }
            let kill_deadline_hit = sent_kill
                && kill_at.is_some_and(|t| now.saturating_duration_since(t) >= kill_after);
            let group_gone = !process_group_live(state.pgid);
            let leader_gone = leader_has_exited(state.pid);
            let finished = (leader_gone && group_gone) || kill_deadline_hit;
            if finished {
                if status.is_none()
                    && let Ok(s) = state.child.wait()
                {
                    record_wait();
                    status = Some(s);
                }
                let reader_failed = join_readers(stdout_thread, stderr_thread);
                let report = OwnerCompletion {
                    status,
                    stdout: state.stdout_log.snapshot(),
                    stderr: state.stderr_log.snapshot(),
                    reader_failed,
                };
                finish_owner(&state, report, done_ack.take());
                return;
            }
            continue;
        }

        if !leader_live {
            if status.is_none() {
                if process_group_live(state.pgid) {
                    if !sent_kill {
                        terminate_group_kill(Some(state.pgid));
                        sent_kill = true;
                    }
                    continue;
                }
                if let Ok(s) = state.child.wait() {
                    record_wait();
                    status = Some(s);
                }
            }
            let _ = wait_reaped(state.pid, Duration::from_secs(2));
            let reader_failed = join_readers(stdout_thread, stderr_thread);
            let report = OwnerCompletion {
                status,
                stdout: state.stdout_log.snapshot(),
                stderr: state.stderr_log.snapshot(),
                reader_failed,
            };
            finish_owner(&state, report, done_ack.take());
            return;
        }
    }
}

fn finish_owner(
    state: &OwnerState,
    report: OwnerCompletion,
    ack: Option<oneshot::Sender<OwnerCompletion>>,
) {
    let _ = wait_reaped(state.pid, Duration::from_secs(2));
    *state
        .completion
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(report.clone());
    state.reaped.store(true, Ordering::SeqCst);
    state.leader_alive.store(false, Ordering::SeqCst);
    state.registry.unregister(state.registry_id);
    if let Some(ack) = ack {
        let _ = ack.send(report);
    }
}

fn leader_is_live(pid: i32) -> bool {
    !leader_has_exited(pid)
}

fn leader_has_exited(pid: i32) -> bool {
    let Some(raw) = Pid::from_raw(pid) else {
        return true;
    };
    match waitid(
        WaitId::Pid(raw),
        WaitIdOptions::NOHANG | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
    ) {
        Ok(Some(status)) => status.exited() || status.killed() || status.dumped(),
        _ => false,
    }
}

fn join_readers(
    stdout: Option<JoinHandle<io::Result<()>>>,
    stderr: Option<JoinHandle<io::Result<()>>>,
) -> bool {
    let mut failed = false;
    if let Some(t) = stdout {
        let result = t.join();
        let err = result.is_err() || result.ok().is_some_and(|r| r.is_err());
        record_reader_join(err);
        failed |= err;
    }
    if let Some(t) = stderr {
        let result = t.join();
        let err = result.is_err() || result.ok().is_some_and(|r| r.is_err());
        record_reader_join(err);
        failed |= err;
    }
    failed
}

#[cfg(any(test, feature = "test-support"))]
static WAIT_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(any(test, feature = "test-support"))]
static READER_JOIN_ERRORS: AtomicU64 = AtomicU64::new(0);

fn record_wait() {
    #[cfg(any(test, feature = "test-support"))]
    WAIT_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn record_reader_join(had_error: bool) {
    #[cfg(any(test, feature = "test-support"))]
    if had_error {
        READER_JOIN_ERRORS.fetch_add(1, Ordering::SeqCst);
    }
    let _ = had_error;
}

/// Number of `Child::wait`/`try_wait` reaps performed by the owner.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn take_owner_wait_count() -> u64 {
    WAIT_COUNT.swap(0, Ordering::SeqCst)
}

/// Reader join failures retained by the owner (never discarded silently).
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn take_reader_join_errors() -> u64 {
    READER_JOIN_ERRORS.swap(0, Ordering::SeqCst)
}
