//! Bounded stdout/stderr capture with secret and header redaction.

use open_compute_core::Redactor;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MAX_LINE: usize = 8 * 1024;
const MAX_TAIL: usize = 16 * 1024;

/// Redacted, bounded process output tail.
#[derive(Clone, Debug, Default)]
pub(crate) struct LogTail {
    bytes: Vec<u8>,
}

impl LogTail {
    /// Redacted UTF-8 lossy view. Never used in public Debug of the supervisor.
    #[must_use]
    pub(crate) fn as_lossy_str(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

pub(crate) struct LogCollector {
    inner: Arc<Mutex<LogTail>>,
    redactor: Redactor,
}

impl Clone for LogCollector {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            redactor: self.redactor.clone(),
        }
    }
}

impl LogCollector {
    pub(crate) fn new(redactor: Redactor) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogTail::default())),
            redactor,
        }
    }

    pub(crate) fn snapshot(&self) -> LogTail {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn ingest(&self, chunk: &[u8]) {
        let redacted = redact_secret_like(&self.redactor, chunk);
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        append_bounded(&mut guard.bytes, &redacted);
    }
}

pub(crate) fn read_pipe_into<R: Read>(mut pipe: R, collector: &LogCollector) -> io::Result<()> {
    if reader_fail_point() {
        return Err(io::Error::other("injected reader failure"));
    }
    let mut buf = [0u8; 4096];
    let mut pending = Vec::new();
    loop {
        let n = match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        pending.extend_from_slice(&buf[..n]);
        while let Some(idx) = pending.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = pending.drain(..=idx).collect();
            if line.len() > MAX_LINE {
                line = b"<truncated-line>\n".to_vec();
            }
            collector.ingest(&line);
        }
        if pending.len() > MAX_LINE {
            collector.ingest(b"<truncated-line>\n");
            pending.clear();
        }
    }
    if !pending.is_empty() {
        if pending.len() > MAX_LINE {
            collector.ingest(b"<truncated-line>");
        } else {
            collector.ingest(&pending);
        }
    }
    Ok(())
}

fn append_bounded(dst: &mut Vec<u8>, add: &[u8]) {
    dst.extend_from_slice(add);
    if dst.len() > MAX_TAIL {
        let drop = dst.len() - MAX_TAIL;
        dst.drain(..drop);
    }
}

fn redact_secret_like(redactor: &Redactor, input: &[u8]) -> Vec<u8> {
    let redacted = redactor.redact_bytes(input);
    let lossy = String::from_utf8_lossy(&redacted);
    let mut out = String::with_capacity(lossy.len());
    for line in lossy.split_inclusive('\n') {
        if let Some((name, _)) = line.split_once(':') {
            let key = name.trim();
            if is_secret_header(key) {
                let nl = if line.ends_with('\n') { "\n" } else { "" };
                out.push_str(key);
                out.push_str(": [REDACTED]");
                out.push_str(nl);
                continue;
            }
        }
        out.push_str(line);
    }
    out.into_bytes()
}

static READER_FAIL: AtomicBool = AtomicBool::new(false);

fn reader_fail_point() -> bool {
    READER_FAIL.swap(false, Ordering::SeqCst)
}

/// Inject a one-shot stdout/stderr reader failure.
#[cfg(any(test, feature = "test-support"))]
pub fn set_reader_fail_point() {
    READER_FAIL.store(true, Ordering::SeqCst);
}

fn is_secret_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "x-api-key"
            | "x-open-compute-internal-token"
    )
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
