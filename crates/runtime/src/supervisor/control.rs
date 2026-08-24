//! Bounded control-fd listen-event parser.

use open_compute_core::{ErrorCode, PlatformError};
use serde::Deserialize;
use std::net::{Ipv4Addr, SocketAddrV4};

const MAX_LINE: usize = 4096;
const MAX_TOTAL: usize = 16 * 1024;

/// Successful loopback HTTP listen report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListenEvent {
    /// Ephemeral port assigned by the OS.
    pub(crate) port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenMessage {
    event: String,
    socket: String,
    port: u64,
    #[serde(default)]
    address: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ControlParser {
    buf: Vec<u8>,
    seen: bool,
    listen: Option<ListenEvent>,
    total: usize,
}

impl ControlParser {
    pub(crate) fn new() -> Self {
        Self {
            buf: Vec::new(),
            seen: false,
            listen: None,
            total: 0,
        }
    }

    pub(crate) fn listen(&self) -> Option<ListenEvent> {
        self.listen
    }

    pub(crate) fn accepted(&self) -> bool {
        self.seen
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<(), PlatformError> {
        self.total = self.total.saturating_add(chunk.len());
        if self.total > MAX_TOTAL {
            return Err(malformed("control-fd output exceeded the size bound"));
        }
        if self.buf.len().saturating_add(chunk.len()) > MAX_TOTAL {
            return Err(malformed("control-fd output exceeded the size bound"));
        }
        self.buf.extend_from_slice(chunk);
        loop {
            let Some(idx) = self.buf.iter().position(|b| *b == b'\n') else {
                if self.buf.len() > MAX_LINE {
                    return Err(malformed("control-fd line exceeded the size bound"));
                }
                break;
            };
            let mut line: Vec<u8> = self.buf.drain(..=idx).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                continue;
            }
            if line.len() > MAX_LINE {
                return Err(malformed("control-fd line exceeded the size bound"));
            }
            self.handle_line(&line)?;
        }
        Ok(())
    }
}

impl ControlParser {
    fn handle_line(&mut self, line: &[u8]) -> Result<(), PlatformError> {
        let text = std::str::from_utf8(line)
            .map_err(|_| malformed("control-fd line is not valid UTF-8"))?;
        let msg: ListenMessage = serde_json::from_str(text)
            .map_err(|_| malformed("control-fd line is not a valid listen message"))?;
        if msg.event != "listen" {
            return Err(malformed(
                "control-fd event is not the expected listen event",
            ));
        }
        if msg.socket != "http" {
            return Err(malformed("control-fd listen socket is not http"));
        }
        if msg.port == 0 || msg.port > u64::from(u16::MAX) {
            return Err(malformed("control-fd listen port is invalid"));
        }
        let port =
            u16::try_from(msg.port).map_err(|_| malformed("control-fd listen port is invalid"))?;
        if let Some(addr) = msg.address.as_deref() {
            validate_address(addr, port)?;
        }
        if self.seen {
            return Err(malformed("control-fd emitted a duplicate listen event"));
        }
        self.seen = true;
        self.listen = Some(ListenEvent { port });
        Ok(())
    }
}

fn validate_address(text: &str, port: u16) -> Result<(), PlatformError> {
    let sock: SocketAddrV4 = text
        .parse()
        .map_err(|_| malformed("control-fd listen address is not a valid IPv4 socket address"))?;
    if *sock.ip() != Ipv4Addr::LOCALHOST {
        return Err(malformed("control-fd listen address is not loopback"));
    }
    if sock.port() != port {
        return Err(malformed(
            "control-fd listen address port does not match the listen port",
        ));
    }
    Ok(())
}

fn malformed(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeExitedBeforeReady, message)
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
