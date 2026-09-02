//! Isolated parser child lifecycle, resource fencing, and bounded protocol I/O.

use open_compute_core::ErrorCode;
use open_compute_document_parser::MAX_OUTPUT_FRAME_BYTES;
use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    run_parser_child_inner(
        executable,
        frame,
        deadline,
        max_stderr,
        max_address_space_bytes,
        max_cpu_seconds,
        working_dir.path(),
    )
    .await
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

async fn run_parser_child_inner(
    executable: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
    working_dir: &Path,
) -> Result<Vec<u8>, ErrorCode> {
    let mut command = Command::new(executable);
    command
        .arg("__document-parser-v1")
        .arg(max_address_space_bytes.to_string())
        .arg(max_cpu_seconds.to_string())
        .env_clear()
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
        .map_err(|_| ErrorCode::DocumentUnavailable)?;
    let pid = child.id().and_then(|pid| i32::try_from(pid).ok());
    let mut process_guard = ParserProcessGuard::new(pid);
    let Some(mut stdin) = child.stdin.take() else {
        terminate_parser(&mut child, &mut process_guard).await;
        return Err(ErrorCode::DocumentUnavailable);
    };
    let Some(stdout) = child.stdout.take() else {
        terminate_parser(&mut child, &mut process_guard).await;
        return Err(ErrorCode::DocumentUnavailable);
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_parser(&mut child, &mut process_guard).await;
        return Err(ErrorCode::DocumentUnavailable);
    };
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(u64::try_from(MAX_OUTPUT_FRAME_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(u64::try_from(max_stderr + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    if stdin.write_all(&frame).await.is_err() || stdin.shutdown().await.is_err() {
        terminate_parser(&mut child, &mut process_guard).await;
        return Err(ErrorCode::DocumentUnavailable);
    }
    drop(stdin);
    let status = match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) => {
            terminate_parser(&mut child, &mut process_guard).await;
            return Err(ErrorCode::DocumentUnavailable);
        }
        Err(_) => {
            terminate_parser(&mut child, &mut process_guard).await;
            return Err(ErrorCode::DocumentTimeout);
        }
    };
    process_guard.disarm();
    let output = stdout_task
        .await
        .map_err(|_| ErrorCode::DocumentUnavailable)?
        .map_err(|_| ErrorCode::DocumentUnavailable)?;
    let stderr = stderr_task
        .await
        .map_err(|_| ErrorCode::DocumentUnavailable)?
        .map_err(|_| ErrorCode::DocumentUnavailable)?;
    if !status.success() || !stderr.is_empty() || output.len() > MAX_OUTPUT_FRAME_BYTES {
        return Err(ErrorCode::DocumentUnavailable);
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
