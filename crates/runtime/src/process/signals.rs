use super::*;

pub(crate) fn terminate_group_term(pid: Option<i32>) {
    if let Some(raw) = pid
        && let Some(pid) = Pid::from_raw(raw)
    {
        record_signal(raw, "TERM");
        let _ = kill_process_group(pid, Signal::TERM);
    }
}

pub(crate) fn terminate_group_kill(pid: Option<i32>) {
    if let Some(raw) = pid
        && let Some(pid) = Pid::from_raw(raw)
    {
        record_signal(raw, "KILL");
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = kill_process(pid, Signal::KILL);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(super) static SIGNAL_LOG: Mutex<Vec<(i32, &'static str)>> = Mutex::new(Vec::new());

pub(crate) fn record_kill_target(pid: i32) {
    record_signal(pid, "KILL");
}

pub(super) fn record_signal(pid: i32, kind: &'static str) {
    #[cfg(any(test, feature = "test-support"))]
    {
        SIGNAL_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((pid, kind));
    }
    let _ = (pid, kind);
}

/// Recorded TERM/KILL targets. Test-support only.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn take_signal_log() -> Vec<(i32, &'static str)> {
    std::mem::take(
        &mut *SIGNAL_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Clear recorded TERM/KILL targets.
#[cfg(any(test, feature = "test-support"))]
pub fn clear_signal_log() {
    SIGNAL_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Assert a PID and its process group are gone.
pub fn assert_reaped(pid: Option<i32>) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if REAP_PROBE_FAIL.load(Ordering::SeqCst) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "injected child reap probe failure",
        ));
    }
    let Some(raw) = pid else {
        return Ok(());
    };
    let Some(pid) = Pid::from_raw(raw) else {
        return Ok(());
    };
    match test_kill_process(pid) {
        Err(err) if err == rustix::io::Errno::SRCH => {}
        Ok(()) => {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process state could not be verified",
            ));
        }
    }
    match test_kill_process_group(pid) {
        Err(err) if err == rustix::io::Errno::SRCH => Ok(()),
        Ok(()) => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "child process group was not reaped",
        )),
        Err(_) => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "child process group state could not be verified",
        )),
    }
}

/// Inject a fail-closed reap probe error. Test-support only.
#[cfg(any(test, feature = "test-support"))]
pub fn set_reap_probe_fail(fail: bool) {
    REAP_PROBE_FAIL.store(fail, Ordering::SeqCst);
}

/// Wait until `pid` is gone or `deadline` elapses.
pub fn wait_pid_gone(pid: i32, deadline: Duration) -> Result<(), PlatformError> {
    let started = std::time::Instant::now();
    let Some(raw) = Pid::from_raw(pid) else {
        return Ok(());
    };
    loop {
        match test_kill_process(raw) {
            Err(err) if err == rustix::io::Errno::SRCH => return Ok(()),
            _ => {}
        }
        if started.elapsed() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait until `pid` and its process group are gone or `deadline` elapses.
pub fn wait_reaped(pid: i32, deadline: Duration) -> Result<(), PlatformError> {
    let started = std::time::Instant::now();
    loop {
        if assert_reaped(Some(pid)).is_ok() {
            return Ok(());
        }
        if started.elapsed() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
pub(super) static EXEC_HOOK: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
pub(super) static OWNER_REAPED_HOOK: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
pub(super) static OWNER_SPAWN_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static OWNER_SPAWN_HOOK: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>> =
    Mutex::new(None);

#[cfg(test)]
pub(super) static STDOUT_READ_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static STDERR_READ_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static STDOUT_WRITE_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static STDOUT_FLUSH_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static STDOUT_SYNC_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) type PgidVerifyHook = Arc<dyn Fn(i32) -> bool + Send + Sync>;

#[cfg(test)]
pub(super) static PGID_VERIFY_HOOK: Mutex<Option<PgidVerifyHook>> = Mutex::new(None);

#[cfg(any(test, feature = "test-support"))]
pub(super) static REAP_PROBE_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static WAIT_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(super) static WAIT_FAIL_HOOK: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>> =
    Mutex::new(None);

#[cfg(test)]
pub(super) static READER_PANIC: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn set_exec_hook(hook: impl Fn() + Send + Sync + 'static) {
    *EXEC_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_exec_hook() {
    *EXEC_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

pub(super) fn run_exec_hook() {
    #[cfg(test)]
    {
        if let Some(hook) = EXEC_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook();
        }
    }
}

pub(super) fn run_owner_reaped_hook() {
    #[cfg(test)]
    {
        if let Some(hook) = OWNER_REAPED_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook();
        }
    }
}

pub(super) fn owner_spawn_should_fail() -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = OWNER_SPAWN_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook();
        }
        OWNER_SPAWN_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn stdout_read_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_READ_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn stderr_read_should_fail() -> bool {
    #[cfg(test)]
    {
        STDERR_READ_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn stdout_write_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_WRITE_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn stdout_flush_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_FLUSH_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn stdout_sync_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_SYNC_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn pgid_verify_should_fail(_pid: i32) -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = PGID_VERIFY_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook(_pid);
        }
        false
    }
    #[cfg(not(test))]
    false
}

pub(super) fn wait_should_fail() -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = WAIT_FAIL_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook();
        }
        WAIT_FAIL.load(Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

pub(super) fn reader_panic_hook() {
    #[cfg(test)]
    {
        if READER_PANIC.swap(false, Ordering::SeqCst) {
            panic!("test stdout reader panic");
        }
    }
}

#[cfg(test)]
pub(crate) fn set_owner_reaped_hook(hook: impl Fn() + Send + Sync + 'static) {
    *OWNER_REAPED_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_owner_reaped_hook() {
    *OWNER_REAPED_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
pub(crate) fn set_owner_spawn_fail_hook(hook: impl Fn() -> bool + Send + Sync + 'static) {
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_owner_spawn_fail_hook() {
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    OWNER_SPAWN_FAIL.store(false, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_stdout_write_fail(fail: bool) {
    STDOUT_WRITE_FAIL.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_pgid_verify_fail_hook(hook: impl Fn(i32) -> bool + Send + Sync + 'static) {
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_pgid_verify_fail_hook() {
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
pub(crate) fn set_wait_fail_hook(hook: impl Fn() -> bool + Send + Sync + 'static) {
    *WAIT_FAIL_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn set_reader_panic(fail: bool) {
    READER_PANIC.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_stdout_read_fail(fail: bool) {
    STDOUT_READ_FAIL.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn clear_io_fail_hooks() {
    OWNER_SPAWN_FAIL.store(false, Ordering::SeqCst);
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    STDOUT_READ_FAIL.store(false, Ordering::SeqCst);
    STDERR_READ_FAIL.store(false, Ordering::SeqCst);
    STDOUT_WRITE_FAIL.store(false, Ordering::SeqCst);
    STDOUT_FLUSH_FAIL.store(false, Ordering::SeqCst);
    STDOUT_SYNC_FAIL.store(false, Ordering::SeqCst);
    WAIT_FAIL.store(false, Ordering::SeqCst);
    *WAIT_FAIL_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    READER_PANIC.store(false, Ordering::SeqCst);
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
