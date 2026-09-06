//! Public WebSocket handshakes and a bounded, transparent upgrade tunnel.

use super::*;
use hyper_util::rt::TokioIo;
use sha1::{Digest, Sha1};

pub(super) struct WebSocketHandshake {
    downstream: hyper::upgrade::OnUpgrade,
    accept: String,
}

fn token(headers: &HeaderMap, name: HeaderName, expected: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case(expected))
}

impl WebSocketHandshake {
    pub(super) fn capture(request: &mut Request) -> Result<Option<Self>, StatusCode> {
        if !token(request.headers(), header::UPGRADE, "websocket") {
            return Ok(None);
        }
        let headers = request.headers();
        let key = headers
            .get(header::SEC_WEBSOCKET_KEY)
            .ok_or(StatusCode::BAD_REQUEST)?;
        if request.method() != Method::GET
            || !token(headers, header::CONNECTION, "upgrade")
            || headers
                .get(header::SEC_WEBSOCKET_VERSION)
                .is_none_or(|value| value != "13")
            || headers.get_all(header::SEC_WEBSOCKET_KEY).iter().count() != 1
            || !base64::engine::general_purpose::STANDARD
                .decode(key.as_bytes())
                .is_ok_and(|bytes| bytes.len() == 16)
            || request.body().size_hint().lower() != 0
            || headers.contains_key(header::TRANSFER_ENCODING)
            || headers
                .get(header::CONTENT_LENGTH)
                .is_some_and(|value| value != "0")
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut hash = Sha1::new();
        hash.update(key.as_bytes());
        hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        let accept = base64::engine::general_purpose::STANDARD.encode(hash.finalize());
        Ok(Some(Self {
            downstream: hyper::upgrade::on(request),
            accept,
        }))
    }

    pub(super) fn connect(
        self,
        response: &mut hyper::Response<hyper::body::Incoming>,
    ) -> Result<(), PlatformError> {
        if !token(response.headers(), header::UPGRADE, "websocket")
            || !token(response.headers(), header::CONNECTION, "upgrade")
            || response
                .headers()
                .get(header::SEC_WEBSOCKET_ACCEPT)
                .is_none_or(|value| value.as_bytes() != self.accept.as_bytes())
        {
            return Err(runtime_unavailable());
        }
        let upstream = hyper::upgrade::on(response);
        tokio::spawn(async move {
            // The header deadline also bounds a client disappearing during upgrade.
            let ready = tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, async {
                tokio::try_join!(self.downstream, upstream)
            })
            .await;
            if let Ok(Ok((downstream, upstream))) = ready {
                // Tokio keeps one fixed-size buffer per direction and propagates EOF.
                // workerd owns WebSocket framing, limits, close semantics and lifecycle.
                let _ = tokio::io::copy_bidirectional(
                    &mut TokioIo::new(downstream),
                    &mut TokioIo::new(upstream),
                )
                .await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
#[path = "websocket_tests.rs"]
mod tests;
