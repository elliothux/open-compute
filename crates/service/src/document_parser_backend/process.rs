//! Isolated parser child lifecycle, resource fencing, and bounded protocol I/O.

use open_compute_core::ErrorCode;
use open_compute_document_parser::MAX_OUTPUT_FRAME_BYTES;
use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use sha2::{Digest as _, Sha256};
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::Command;
use uuid::Uuid;

pub(super) async fn run_parser_child(
    executable: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
) -> Result<Vec<u8>, ErrorCode> {
    let working_dir = ParserWorkingDirectory::create()?;
    match run_parser_child_inner(
        executable,
        frame,
        deadline,
        max_stderr,
        max_address_space_bytes,
        max_cpu_seconds,
        working_dir.path(),
    )
    .await
    {
        Ok(output) => Ok(output),
        Err(failure) => {
            failure.report();
            Err(failure.error_code())
        }
    }
}

struct ParserWorkingDirectory {
    path: PathBuf,
}

impl ParserWorkingDirectory {
    fn create() -> Result<Self, ErrorCode> {
        let path =
            std::env::temp_dir().join(format!("open-compute-document-parser-{}", Uuid::now_v7()));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&path)
            .map_err(|_| ErrorCode::DocumentUnavailable)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ParserWorkingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ParserProcessGuard {
    pid: Option<i32>,
    armed: bool,
}

impl ParserProcessGuard {
    fn new(pid: Option<i32>) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ParserProcessGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_parser_group(self.pid);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ParserFailureKind {
    Spawn,
    Stream,
    InputIo,
    WaitIo,
    OutputIo,
    TimedOut,
    ProcessExited,
    StdoutLimit,
    StderrLimit,
}

impl ParserFailureKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Stream => "stream",
            Self::InputIo => "input_io",
            Self::WaitIo => "wait_io",
            Self::OutputIo => "output_io",
            Self::TimedOut => "timeout",
            Self::ProcessExited => "process_exit",
            Self::StdoutLimit => "stdout_limit",
            Self::StderrLimit => "stderr_limit",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ParserChildFailure {
    kind: ParserFailureKind,
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stderr_sha256: Option<[u8; 32]>,
}

impl ParserChildFailure {
    fn empty(kind: ParserFailureKind) -> Self {
        Self {
            kind,
            exit_code: None,
            signal: None,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stderr_sha256: None,
        }
    }

    fn observed(
        kind: ParserFailureKind,
        status: Option<&ExitStatus>,
        output: &CapturedOutput,
    ) -> Self {
        Self {
            kind,
            exit_code: status.and_then(ExitStatus::code),
            signal: status.and_then(ExitStatusExt::signal),
            stdout_bytes: output.stdout.len(),
            stderr_bytes: output.stderr.len(),
            stderr_sha256: (!output.stderr.is_empty())
                .then(|| Sha256::digest(&output.stderr).into()),
        }
    }

    #[cfg(test)]
    pub(super) const fn kind(&self) -> ParserFailureKind {
        self.kind
    }

    #[cfg(test)]
    pub(super) const fn signal(&self) -> Option<i32> {
        self.signal
    }

    #[cfg(test)]
    pub(super) const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    const fn error_code(&self) -> ErrorCode {
        match self.kind {
            ParserFailureKind::TimedOut => ErrorCode::DocumentTimeout,
            ParserFailureKind::ProcessExited
            | ParserFailureKind::StdoutLimit
            | ParserFailureKind::StderrLimit => ErrorCode::DocumentProcessFailed,
            ParserFailureKind::Spawn
            | ParserFailureKind::Stream
            | ParserFailureKind::InputIo
            | ParserFailureKind::WaitIo
            | ParserFailureKind::OutputIo => ErrorCode::DocumentUnavailable,
        }
    }

    fn report(&self) {
        let stderr_sha256 = self.stderr_sha256.map(hex::encode).unwrap_or_default();
        tracing::warn!(
            failure = self.kind.as_str(),
            exit_code = ?self.exit_code,
            signal = ?self.signal,
            stdout_bytes = self.stdout_bytes,
            stderr_bytes = self.stderr_bytes,
            stderr_sha256,
            "isolated document parser child failed"
        );
    }
}

#[derive(Debug)]
pub(super) struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub(super) async fn run_parser_child_inner(
    executable: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
    working_dir: &Path,
) -> Result<Vec<u8>, ParserChildFailure> {
    let mut command = Command::new(executable);
    command
        .arg("__document-parser-v1")
        .arg(max_address_space_bytes.to_string())
        .arg(max_cpu_seconds.to_string())
        .env_clear()
        // Keep every upstream cache resolution inside the disposable sandbox.
        // OCR result caching is disabled, so this directory must remain unused.
        .env("XBERG_CACHE_DIR", working_dir.join("xberg-cache"))
        // Keep compiler-inserted profiling runtimes from attempting a regular-file
        // write after the child has installed its zero-byte file-size limit.
        .env("LLVM_PROFILE_FILE", "/dev/null")
        .current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| ParserChildFailure::empty(ParserFailureKind::Spawn))?;
    let pid = child.id().and_then(|pid| i32::try_from(pid).ok());
    let mut process_guard = ParserProcessGuard::new(pid);
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        terminate_parser(&mut child, &mut process_guard).await;
        return Err(ParserChildFailure::empty(ParserFailureKind::Stream));
    };
    let stdout_task = tokio::spawn(async move {
        read_bounded(stdout, MAX_OUTPUT_FRAME_BYTES.saturating_add(1)).await
    });
    let stderr_task =
        tokio::spawn(async move { read_bounded(stderr, max_stderr.saturating_add(1)).await });
    let input_task = tokio::spawn(async move {
        stdin.write_all(&frame).await?;
        stdin.shutdown().await
    });

    let status = match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(_)) => {
            terminate_parser(&mut child, &mut process_guard).await;
            None
        }
        Err(_) => {
            terminate_parser(&mut child, &mut process_guard).await;
            let (output, _) = tokio::join!(collect_output(stdout_task, stderr_task), input_task);
            let output = output?;
            return Err(ParserChildFailure::observed(
                ParserFailureKind::TimedOut,
                None,
                &output,
            ));
        }
    };
    process_guard.disarm();
    let (output, input_result) = tokio::join!(collect_output(stdout_task, stderr_task), input_task);
    let output = output?;
    let input_ok = matches!(input_result, Ok(Ok(())));
    let Some(status) = status else {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::WaitIo,
            None,
            &output,
        ));
    };
    if output.stdout.len() > MAX_OUTPUT_FRAME_BYTES {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::StdoutLimit,
            Some(&status),
            &output,
        ));
    }
    if output.stderr.len() > max_stderr {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::StderrLimit,
            Some(&status),
            &output,
        ));
    }
    if !status.success() {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::ProcessExited,
            Some(&status),
            &output,
        ));
    }
    if !input_ok {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::InputIo,
            Some(&status),
            &output,
        ));
    }
    Ok(output.stdout)
}

async fn read_bounded(reader: impl tokio::io::AsyncRead + Unpin, limit: usize) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let success = reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await
        .is_ok();
    (bytes, success)
}

pub(super) async fn collect_output(
    stdout_task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
    stderr_task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
) -> Result<CapturedOutput, ParserChildFailure> {
    let (stdout, stderr) = tokio::join!(stdout_task, stderr_task);
    let (stdout, stdout_ok) =
        stdout.map_err(|_| ParserChildFailure::empty(ParserFailureKind::OutputIo))?;
    let (stderr, stderr_ok) =
        stderr.map_err(|_| ParserChildFailure::empty(ParserFailureKind::OutputIo))?;
    let output = CapturedOutput { stdout, stderr };
    if !stdout_ok || !stderr_ok {
        return Err(ParserChildFailure::observed(
            ParserFailureKind::OutputIo,
            None,
            &output,
        ));
    }
    Ok(output)
}

async fn terminate_parser(child: &mut tokio::process::Child, guard: &mut ParserProcessGuard) {
    kill_parser_group(guard.pid);
    let _ = child.kill().await;
    let _ = child.wait().await;
    guard.disarm();
}

fn kill_parser_group(pid: Option<i32>) {
    if let Some(pid) = pid.and_then(Pid::from_raw) {
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = kill_process(pid, Signal::KILL);
    }
}
