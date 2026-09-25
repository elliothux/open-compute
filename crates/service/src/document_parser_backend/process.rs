//! Isolated parser child lifecycle, resource fencing, and bounded protocol I/O.

use open_compute_core::{ErrorCode, Redactor};
use open_compute_document_parser::MAX_OUTPUT_FRAME_BYTES;
use open_compute_runtime::{
    HostProcessLease, HostProcessSpec, VerifiedLaunchImage, run_host_process,
};
use sha2::{Digest as _, Sha256};
use std::ffi::OsString;
#[cfg(test)]
use std::fs::File;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

#[allow(
    clippy::too_many_arguments,
    reason = "parser execution limits stay explicit"
)]
pub(super) async fn run_parser_child(
    executable: &VerifiedLaunchImage,
    executable_sha256: &str,
    tmp_root: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
) -> Result<Vec<u8>, ErrorCode> {
    let working_dir = ParserWorkingDirectory::create(tmp_root)?;
    let result = run_parser_image(
        executable,
        executable_sha256,
        frame,
        deadline,
        max_stderr,
        max_address_space_bytes,
        max_cpu_seconds,
        working_dir.path(),
    )
    .await;
    crate::task_workspace::mark_completed(working_dir.path())
        .map_err(|_| ErrorCode::DocumentUnavailable)?;
    match result {
        Ok(output) => Ok(output),
        Err(failure) => {
            failure.report();
            Err(failure.error_code())
        }
    }
}

struct ParserWorkingDirectory {
    workspace: tempfile::TempDir,
}

impl ParserWorkingDirectory {
    fn create(tmp_root: &Path) -> Result<Self, ErrorCode> {
        let workspace = crate::task_workspace::create(tmp_root, "document-parser-")
            .map_err(|_| ErrorCode::DocumentUnavailable)?;
        Ok(Self { workspace })
    }

    fn path(&self) -> &Path {
        self.workspace.path()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ParserFailureKind {
    #[cfg(test)]
    Spawn,
    InputIo,
    OutputIo,
    TimedOut,
    ProcessExited,
    StdoutLimit,
    StderrLimit,
}

impl ParserFailureKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            #[cfg(test)]
            Self::Spawn => "spawn",
            Self::InputIo => "input_io",
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
            ParserFailureKind::InputIo | ParserFailureKind::OutputIo => {
                ErrorCode::DocumentUnavailable
            }
            #[cfg(test)]
            ParserFailureKind::Spawn => ErrorCode::DocumentUnavailable,
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
struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "parser execution limits stay explicit"
)]
async fn run_parser_image(
    executable: &VerifiedLaunchImage,
    executable_sha256: &str,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
    working_dir: &Path,
) -> Result<Vec<u8>, ParserChildFailure> {
    let output = run_host_process(
        executable,
        HostProcessSpec {
            args: vec![
                OsString::from("__document-parser-v1"),
                OsString::from(max_address_space_bytes.to_string()),
                OsString::from(max_cpu_seconds.to_string()),
            ],
            environment: vec![
                (OsString::from("HOME"), working_dir.as_os_str().to_owned()),
                (
                    OsString::from("XDG_CACHE_HOME"),
                    working_dir.as_os_str().to_owned(),
                ),
                (
                    OsString::from("XDG_CONFIG_HOME"),
                    working_dir.as_os_str().to_owned(),
                ),
                (
                    OsString::from("XDG_DATA_HOME"),
                    working_dir.as_os_str().to_owned(),
                ),
                (OsString::from("TMPDIR"), working_dir.as_os_str().to_owned()),
                (OsString::from("TMP"), working_dir.as_os_str().to_owned()),
                (OsString::from("TEMP"), working_dir.as_os_str().to_owned()),
                (
                    OsString::from("XBERG_CACHE_DIR"),
                    working_dir.join("xberg-cache").into_os_string(),
                ),
                (
                    OsString::from("LLVM_PROFILE_FILE"),
                    OsString::from("/dev/null"),
                ),
            ],
            working_directory: working_dir.to_owned(),
            stdin: frame,
            deadline,
            max_stdout: MAX_OUTPUT_FRAME_BYTES,
            max_stderr,
            redactor: Redactor::new(),
            lease: Some(HostProcessLease {
                path: working_dir.join("child.lease"),
                binary_sha256: executable_sha256.to_owned(),
            }),
        },
    )
    .await
    .map_err(|_| ParserChildFailure::empty(ParserFailureKind::OutputIo))?;
    let captured = CapturedOutput {
        stdout: output.stdout,
        stderr: output.stderr,
    };
    let failure = if output.timed_out {
        Some(ParserFailureKind::TimedOut)
    } else if output.stdout_overflow {
        Some(ParserFailureKind::StdoutLimit)
    } else if output.stderr_overflow {
        Some(ParserFailureKind::StderrLimit)
    } else if output.stdin_error {
        Some(ParserFailureKind::InputIo)
    } else if output.status.is_none_or(|status| !status.success()) {
        Some(ParserFailureKind::ProcessExited)
    } else {
        None
    };
    if let Some(kind) = failure {
        return Err(ParserChildFailure::observed(
            kind,
            output.status.as_ref(),
            &captured,
        ));
    }
    Ok(captured.stdout)
}

#[cfg(test)]
pub(super) async fn run_parser_child_inner(
    executable: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
    working_dir: &Path,
) -> Result<Vec<u8>, ParserChildFailure> {
    let mut file =
        File::open(executable).map_err(|_| ParserChildFailure::empty(ParserFailureKind::Spawn))?;
    let digest = super::digest_executable(&mut file)
        .map_err(|_| ParserChildFailure::empty(ParserFailureKind::Spawn))?;
    run_parser_image(
        &VerifiedLaunchImage::from_verified_file(file),
        &digest,
        frame,
        deadline,
        max_stderr,
        max_address_space_bytes,
        max_cpu_seconds,
        working_dir,
    )
    .await
}

#[cfg(test)]
pub(super) async fn run_parser_child_path(
    executable: &Path,
    frame: Vec<u8>,
    deadline: Duration,
    max_stderr: usize,
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
) -> Result<Vec<u8>, ErrorCode> {
    let tmp_root = executable.parent().ok_or(ErrorCode::DocumentUnavailable)?;
    let working = ParserWorkingDirectory::create(tmp_root)?;
    run_parser_child_inner(
        executable,
        frame,
        deadline,
        max_stderr,
        max_address_space_bytes,
        max_cpu_seconds,
        working.path(),
    )
    .await
    .map_err(|failure| failure.error_code())
}
