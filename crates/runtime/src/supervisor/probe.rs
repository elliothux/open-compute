//! Unix socket readiness probe against the formal system worker.

use open_compute_core::{ErrorCode, PlatformError};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

/// Formal ingress probe contract.
pub const READY_PATH: &str = "/internal/ready";

const MAX_RESPONSE: usize = 8 * 1024;
const MAX_HEADER: usize = 4 * 1024;

pub async fn probe_ready(socket: &Path, deadline: Duration) -> Result<(), PlatformError> {
    probe_path(socket, READY_PATH, deadline).await
}

/// One private GET against a Unix socket internal path, expecting 204 with no body.
pub(super) async fn probe_path(
    socket: &Path,
    path: &str,
    deadline: Duration,
) -> Result<(), PlatformError> {
    timeout(deadline, probe_once(socket, path))
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeExitedBeforeReady,
                "runtime readiness probe timed out",
            )
        })?
}

async fn probe_once(socket: &Path, path: &str) -> Result<(), PlatformError> {
    let mut stream = UnixStream::connect(socket).await.map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeExitedBeforeReady,
            "runtime readiness probe could not connect",
        )
    })?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: runtime\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeExitedBeforeReady,
            "runtime readiness probe write failed",
        )
    })?;
    stream.flush().await.map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeExitedBeforeReady,
            "runtime readiness probe write failed",
        )
    })?;
    let parsed = read_http_response(&mut stream, Duration::from_secs(5)).await?;
    if parsed.status == 204 {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::RuntimeExitedBeforeReady,
            "runtime readiness probe was rejected",
        ))
    }
}

struct ParsedResponse {
    status: u16,
}

async fn read_http_response(
    stream: &mut UnixStream,
    deadline: Duration,
) -> Result<ParsedResponse, PlatformError> {
    let deadline = Instant::now() + deadline;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 256];
    let mut empty_204_headers = false;
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err(probe_err("runtime readiness probe timed out"));
        }
        let read_budget = if empty_204_headers {
            remain.min(Duration::from_millis(250))
        } else {
            remain
        };
        let n = match timeout(read_budget, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => {
                if buf.is_empty() {
                    return Err(probe_err("runtime readiness probe read failed"));
                }
                break;
            }
            Ok(Ok(n)) => n,
            Ok(Err(_)) => return Err(probe_err("runtime readiness probe read failed")),
            Err(_) if empty_204_headers => {
                return Ok(ParsedResponse { status: 204 });
            }
            Err(_) => return Err(probe_err("runtime readiness probe timed out")),
        };
        if buf.len().saturating_add(n) > MAX_RESPONSE {
            return Err(probe_err("runtime readiness probe response exceeded bound"));
        }
        buf.extend_from_slice(&tmp[..n]);
        match try_parse_http(&buf, false)? {
            Some(parsed) => return Ok(parsed),
            None if empty_204_complete(&buf)? => {
                empty_204_headers = true;
            }
            None => {}
        }
    }
    try_parse_http(&buf, true)?.ok_or_else(|| probe_err("runtime readiness probe was rejected"))
}

fn empty_204_complete(buf: &[u8]) -> Result<bool, PlatformError> {
    match try_parse_http(buf, true)? {
        Some(parsed) if parsed.status == 204 => Ok(true),
        Some(_) => Ok(false),
        None => Ok(false),
    }
}

fn try_parse_http(buf: &[u8], eof: bool) -> Result<Option<ParsedResponse>, PlatformError> {
    let Some(header_end) = find_header_end(buf) else {
        if buf.len() > MAX_HEADER {
            return Err(probe_err("runtime readiness probe response exceeded bound"));
        }
        return Ok(None);
    };
    if header_end > MAX_HEADER {
        return Err(probe_err("runtime readiness probe response exceeded bound"));
    }
    let headers = std::str::from_utf8(&buf[..header_end])
        .map_err(|_| probe_err("runtime readiness probe was rejected"))?;
    let mut lines = headers.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| probe_err("runtime readiness probe was rejected"))?;
    let mut parts = status_line.split_whitespace();
    let version = parts
        .next()
        .ok_or_else(|| probe_err("runtime readiness probe was rejected"))?;
    if !version.starts_with("HTTP/1.") {
        return Err(probe_err("runtime readiness probe was rejected"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| probe_err("runtime readiness probe was rejected"))?;
    let mut content_length: Option<usize> = None;
    let mut transfer_encoding = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(probe_err("runtime readiness probe was rejected"));
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        }
        if name.eq_ignore_ascii_case("content-length") {
            let parsed: usize = value
                .parse()
                .map_err(|_| probe_err("runtime readiness probe was rejected"))?;
            if let Some(existing) = content_length
                && existing != parsed
            {
                return Err(probe_err("runtime readiness probe was rejected"));
            }
            content_length = Some(parsed);
        }
    }
    if transfer_encoding {
        return Err(probe_err("runtime readiness probe was rejected"));
    }
    let body_start = header_end + 4;
    if status == 204 {
        if content_length.is_some_and(|n| n != 0) {
            return Err(probe_err("runtime readiness probe was rejected"));
        }
        if buf.len() > body_start {
            return Err(probe_err("runtime readiness probe was rejected"));
        }
        if !eof {
            return Ok(None);
        }
        return Ok(Some(ParsedResponse { status }));
    }
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_RESPONSE {
        return Err(probe_err("runtime readiness probe response exceeded bound"));
    }
    if buf.len() < body_start.saturating_add(content_length) {
        if buf.len() > MAX_RESPONSE {
            return Err(probe_err("runtime readiness probe response exceeded bound"));
        }
        return Ok(None);
    }
    Ok(Some(ParsedResponse { status }))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn probe_err(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeExitedBeforeReady, message)
}

#[cfg(test)]
fn parse_http_response_for_test(buf: &[u8]) -> Result<Option<u16>, PlatformError> {
    Ok(try_parse_http(buf, true)?.map(|p| p.status))
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
