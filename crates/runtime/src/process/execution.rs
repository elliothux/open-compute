use super::*;

pub(crate) async fn run_verified_fd(
    file: &File,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    run_exec_hook();
    let image = exec_image(file)?;
    run_image(&image, args, deadline, max_stdout, redactor, stdout_file).await
}

#[allow(
    clippy::too_many_arguments,
    reason = "runtime boundary inputs keep distinct capabilities explicit"
)]
pub(crate) async fn run_verified_fd_with_lease(
    file: &File,
    lease_path: &Path,
    binary_sha256: &str,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    run_exec_hook();
    let image = exec_image_with_lease(file, lease_path, binary_sha256)?;
    run_image(&image, args, deadline, max_stdout, redactor, stdout_file).await
}

async fn run_image(
    image: &ExecImage,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    let mut std_cmd = std::process::Command::new(&image.program);
    std::os::unix::process::CommandExt::process_group(&mut std_cmd, 0);
    std_cmd
        .args(args.iter().copied().map(OsStr::new))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = std_cmd.spawn().map_err(|_| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "failed to spawn runtime process")
    })?;
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    // `--version` can exit before getpgid. spawn already used process_group(0).
    // An exited, unreaped child can still pass kill(pid, 0). Use its owned wait status.
    match verify_self_pgid(pid) {
        Ok(()) => {}
        Err(err) => {
            if err.message() != "failed to read runtime process group"
                || owned.try_wait()?.is_none()
            {
                return Err(err);
            }
        }
    }
    let stdout = owned.take_stdout();
    let stderr = owned.take_stderr();

    let cancel = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = oneshot::channel();
    let started = std::time::Instant::now();
    let deadline_at = started.checked_add(deadline).ok_or_else(|| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "process deadline overflow")
    })?;
    let hard_deadline = deadline_at.checked_add(KILL_GRACE).unwrap_or(deadline_at);

    let owner_cancel = cancel.clone();
    if owner_spawn_should_fail() {
        drop(owned);
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to start process owner thread",
        ));
    }
    let Ok(owner) = std::thread::Builder::new()
        .name("oc-os-owner".into())
        .spawn(move || {
            let output = owner_wait(OwnerWait {
                owned,
                stdout,
                stderr,
                stdout_file,
                max_stdout,
                cancel: owner_cancel,
                deadline_at,
                hard_deadline,
            });
            run_owner_reaped_hook();
            let _ = done_tx.send(output);
        })
    else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to start process owner thread",
        ));
    };

    let mut guard = ProcessGuard {
        cancel: Some(cancel),
        owner: Some(owner),
    };
    let result = done_rx.await.map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process owner task ended without a result",
        )
    })?;
    guard.disarm();
    match result {
        Ok(mut output) => {
            output.stderr = redactor.redact_bytes(output.stderr.as_slice());
            Ok(output)
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn verify_self_pgid(pid: i32) -> Result<(), PlatformError> {
    if pgid_verify_should_fail(pid) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read runtime process group",
        ));
    }
    let Some(raw) = Pid::from_raw(pid) else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process pid is invalid",
        ));
    };
    let pgid = getpgid(Some(raw)).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read runtime process group",
        )
    })?;
    if pgid.as_raw_nonzero().get() != pid {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process is not its own process group leader",
        ));
    }
    Ok(())
}

/// RAII owner of a spawned process group. Drop KILL/waits unless disarmed.
pub(crate) struct OwnedChild {
    pub(crate) child: Option<std::process::Child>,
    pub(crate) pid: i32,
    pub(crate) disarmed: bool,
}

impl OwnedChild {
    pub(crate) fn new(child: std::process::Child, pid: i32) -> Self {
        Self {
            child: Some(child),
            pid,
            disarmed: false,
        }
    }

    pub(crate) fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|child| child.stdout.take())
    }

    pub(crate) fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.as_mut().and_then(|child| child.stderr.take())
    }

    pub(crate) fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, PlatformError> {
        if wait_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            ));
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        child
            .try_wait()
            .map_err(|_| PlatformError::new(ErrorCode::RuntimeInvalid, "failed to wait for child"))
    }

    pub(crate) fn wait(&mut self) -> Result<Option<std::process::ExitStatus>, PlatformError> {
        if wait_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            ));
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        match child.wait() {
            Ok(status) => {
                self.child.take();
                Ok(Some(status))
            }
            Err(_) => Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            )),
        }
    }

    pub(crate) fn fail_safe_kill(&mut self) {
        terminate_group_kill(Some(self.pid));
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        self.fail_safe_kill();
    }
}

pub(crate) struct OwnerWait {
    pub(crate) owned: OwnedChild,
    pub(crate) stdout: Option<std::process::ChildStdout>,
    pub(crate) stderr: Option<std::process::ChildStderr>,
    pub(crate) stdout_file: Option<File>,
    pub(crate) max_stdout: usize,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) deadline_at: std::time::Instant,
    pub(crate) hard_deadline: std::time::Instant,
}

pub(crate) struct PipeState {
    done: Arc<AtomicBool>,
    error: Arc<Mutex<Option<PlatformError>>>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

pub(super) struct ReaderDoneGuard {
    done: Arc<AtomicBool>,
}

impl Drop for ReaderDoneGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

impl PipeState {
    pub(crate) fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            bytes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn take_error(&self) -> Option<PlatformError> {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn take_bytes(&self) -> Vec<u8> {
        std::mem::take(
            &mut *self
                .bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

pub(super) fn owner_wait(
    OwnerWait {
        mut owned,
        stdout,
        stderr,
        mut stdout_file,
        max_stdout,
        cancel,
        deadline_at,
        hard_deadline,
    }: OwnerWait,
) -> Result<BoundedOutput, PlatformError> {
    let pid = owned.pid;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_state = PipeState::new();
    let stderr_state = PipeState::new();
    let stream_to_file = stdout_file.is_some();

    let stdout_thread = stdout.map(|pipe| {
        let overflow = overflow.clone();
        let state = stdout_state.done.clone();
        let error = stdout_state.error.clone();
        let bytes = stdout_state.bytes.clone();
        let file = stdout_file.take();
        std::thread::spawn(move || {
            let _done = ReaderDoneGuard { done: state };
            reader_panic_hook();
            let result = read_stdout(pipe, file, max_stdout, &overflow);
            match result {
                Ok(out) => {
                    *bytes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = out;
                }
                Err(err) => {
                    *error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(err);
                }
            }
        })
    });
    let stderr_thread = stderr.map(|pipe| {
        let state = stderr_state.done.clone();
        let error = stderr_state.error.clone();
        let bytes = stderr_state.bytes.clone();
        std::thread::spawn(move || {
            let _done = ReaderDoneGuard { done: state };
            let result = read_stderr(pipe);
            match result {
                Ok(out) => {
                    *bytes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = out;
                }
                Err(err) => {
                    *error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(err);
                }
            }
        })
    });

    let mut status = None;
    let mut timed_out = false;
    let mut sent_term = false;
    let mut term_at = None;
    let mut outcome_err = None;

    loop {
        let now = std::time::Instant::now();
        match owned.try_wait() {
            Ok(Some(s)) => status = Some(s),
            Ok(None) => {}
            Err(err) => {
                if outcome_err.is_none() {
                    outcome_err = Some(err);
                }
            }
        }
        if let Some(err) = stdout_state.take_error() {
            outcome_err = Some(err);
        }
        if outcome_err.is_none()
            && let Some(err) = stderr_state.take_error()
        {
            outcome_err = Some(err);
        }

        let overflowed = overflow.load(Ordering::SeqCst);
        let cancelled = cancel.load(Ordering::SeqCst);
        let deadline_hit = now >= deadline_at;
        let group_live = process_group_live(pid);
        let readers_done =
            stdout_state.done.load(Ordering::SeqCst) && stderr_state.done.load(Ordering::SeqCst);
        let leader_exited = status.is_some();
        let stop_live = overflowed || cancelled || deadline_hit || outcome_err.is_some();

        if leader_exited && !group_live && readers_done {
            break;
        }
        if (stop_live || (leader_exited && group_live)) && !sent_term {
            terminate_group_term(Some(pid));
            sent_term = true;
            term_at = Some(now);
            timed_out = cancelled || deadline_hit;
        }
        if sent_term {
            let grace_done = term_at
                .is_some_and(|t| now >= t.checked_add(KILL_GRACE).unwrap_or(hard_deadline))
                || now >= hard_deadline;
            if grace_done {
                terminate_group_kill(Some(pid));
                reap_after_kill(&mut owned, &mut status, &mut outcome_err);
                break;
            }
        }
        if now >= hard_deadline {
            terminate_group_kill(Some(pid));
            reap_after_kill(&mut owned, &mut status, &mut outcome_err);
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    if let Err(err) = join_readers(stdout_thread, stderr_thread, hard_deadline)
        && outcome_err.is_none()
    {
        outcome_err = Some(err);
    }
    if status.is_none() {
        match owned.wait() {
            Ok(s) => status = s,
            Err(err) => {
                if outcome_err.is_none() {
                    outcome_err = Some(err);
                }
            }
        }
    }
    if let Some(err) = stdout_state
        .take_error()
        .or_else(|| stderr_state.take_error())
        && outcome_err.is_none()
    {
        outcome_err = Some(err);
    }
    let stdout_bytes = stdout_state.take_bytes();
    let stderr_bytes = stderr_state.take_bytes();
    if let Some(err) = outcome_err {
        return Err(err);
    }
    if status.is_some() {
        let _ = owned.child.take();
    }
    owned.disarm();

    Ok(BoundedOutput {
        status,
        stdout: if stream_to_file {
            Vec::new()
        } else {
            stdout_bytes
        },
        stderr: stderr_bytes,
        timed_out,
        stdout_overflow: overflow.load(Ordering::SeqCst),
        pid: Some(pid),
    })
}

pub(super) fn reap_after_kill(
    owned: &mut OwnedChild,
    status: &mut Option<std::process::ExitStatus>,
    outcome_err: &mut Option<PlatformError>,
) {
    if status.is_some() {
        return;
    }
    match owned.wait() {
        Ok(s) => *status = s,
        Err(err) => {
            if outcome_err.is_none() {
                *outcome_err = Some(err);
            }
            owned.fail_safe_kill();
        }
    }
}

pub(super) fn join_readers(
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    _hard_deadline: std::time::Instant,
) -> Result<(), PlatformError> {
    let mut panicked = false;
    if let Some(t) = stdout_thread
        && t.join().is_err()
    {
        panicked = true;
    }
    if let Some(t) = stderr_thread
        && t.join().is_err()
    {
        panicked = true;
    }
    if panicked {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime output reader panicked",
        ));
    }
    Ok(())
}

pub(crate) fn process_group_live(pid: i32) -> bool {
    let Some(raw) = Pid::from_raw(pid) else {
        return false;
    };
    test_kill_process_group(raw).is_ok()
}

pub(super) fn read_stdout(
    mut pipe: std::process::ChildStdout,
    mut file: Option<File>,
    max_stdout: usize,
    overflow: &AtomicBool,
) -> Result<Vec<u8>, PlatformError> {
    let mut mem = Vec::new();
    let mut written = 0usize;
    let mut tmp = [0u8; 8192];
    loop {
        if stdout_read_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to read runtime stdout",
            ));
        }
        match pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if written.saturating_add(n) > max_stdout {
                    overflow.store(true, Ordering::SeqCst);
                    let mut drain = [0u8; 8192];
                    loop {
                        match pipe.read(&mut drain) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => {
                                return Err(PlatformError::new(
                                    ErrorCode::RuntimeInvalid,
                                    "failed to read runtime stdout",
                                ));
                            }
                        }
                    }
                    break;
                }
                if let Some(out) = file.as_mut() {
                    write_compile_stdout(out, &tmp[..n])?;
                } else {
                    mem.extend_from_slice(&tmp[..n]);
                }
                written += n;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to read runtime stdout",
                ));
            }
        }
    }
    if let Some(out) = file.as_mut()
        && !overflow.load(Ordering::SeqCst)
    {
        finish_compile_stdout(out)?;
    }
    Ok(mem)
}

pub(super) fn write_compile_stdout(file: &mut File, chunk: &[u8]) -> Result<(), PlatformError> {
    if stdout_write_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to write compile output",
        ));
    }
    file.write_all(chunk).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to write compile output",
        )
    })
}

pub(super) fn finish_compile_stdout(file: &mut File) -> Result<(), PlatformError> {
    if stdout_flush_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to flush compile output",
        ));
    }
    file.flush().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to flush compile output",
        )
    })?;
    if stdout_sync_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to fsync compile output",
        ));
    }
    file.sync_all().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to fsync compile output",
        )
    })
}

pub(super) fn read_stderr(mut pipe: std::process::ChildStderr) -> Result<Vec<u8>, PlatformError> {
    let mut err = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if stderr_read_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to read runtime stderr",
            ));
        }
        match pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if err.len() < MAX_STDERR {
                    let take = (MAX_STDERR - err.len()).min(n);
                    err.extend_from_slice(&tmp[..take]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to read runtime stderr",
                ));
            }
        }
    }
    Ok(err)
}

pub(crate) struct ProcessGuard {
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    pub(crate) owner: Option<std::thread::JoinHandle<()>>,
}

impl ProcessGuard {
    pub(crate) fn disarm(&mut self) {
        self.cancel.take();
        let _ = self.owner.take();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        cancel.store(true, Ordering::SeqCst);
        let _ = self.owner.take();
    }
}
