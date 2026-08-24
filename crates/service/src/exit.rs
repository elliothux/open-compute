//! Stable CLI exit classes and secret-safe failure printing.

use open_compute_core::{ErrorCode, PlatformError};
use std::io::{self, Write};
use std::process::ExitCode;

/// Documented nonzero exit classes for `platformd`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ExitClass {
    /// Successful command.
    Ok = 0,
    /// CLI usage / parse failure.
    Cli = 2,
    /// Config path, parse, or static validation failure.
    Config = 3,
    /// Doctor reported one or more failed checks.
    Doctor = 4,
    /// `run` failed before becoming a live listener process, or shutdown error.
    Run = 5,
}

impl ExitClass {
    /// Process exit code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Map a platform error to an exit class.
#[must_use]
pub fn exit_class_for(code: ErrorCode) -> ExitClass {
    match code {
        ErrorCode::ConfigPathInvalid
        | ErrorCode::ConfigParseFailed
        | ErrorCode::ConfigInvalid
        | ErrorCode::AdminAuthRequired
        | ErrorCode::SecretRefInvalid
        | ErrorCode::PathInvalid
        | ErrorCode::S3PrefixInvalid
        | ErrorCode::CacheBoundsInvalid
        | ErrorCode::LimitInvalid => ExitClass::Config,
        _ => ExitClass::Run,
    }
}

/// Print `CODE: static message` with no extra payload.
pub fn emit_failure(err: &PlatformError, out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "{}: {}", err.code().as_str(), err.message())
}

/// Convert a class to [`ExitCode`].
#[must_use]
pub fn exit_code(class: ExitClass) -> ExitCode {
    ExitCode::from(class.code())
}
