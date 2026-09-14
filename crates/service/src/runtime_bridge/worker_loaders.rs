//! Script-scoped native loader cleanup after persisted deletion and invocation drain.

use super::*;

impl WorkerdTransport {
    /// Revoke every native loader namespace owned by a deleted Script.
    pub async fn revoke_worker_loaders(&self, namespaces: &[String]) -> Result<(), PlatformError> {
        if namespaces.is_empty() {
            return Ok(());
        }
        let endpoint = self.endpoint()?;
        let (port, credential) = (endpoint.port, endpoint.credential);
        let mut evidence = None;
        let result = async {
            for batch in namespaces.chunks(128) {
                let body = serde_json::to_vec(batch).map_err(|_| runtime_unavailable())?;
                let request = hyper::Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "http://127.0.0.1:{port}/internal/worker-loaders/revoke"
                    ))
                    .header(TOKEN_HEADER, credential.expose())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .map_err(|_| runtime_unavailable())?;
                let response = match tokio::time::timeout(
                    RESPONSE_HEADER_TIMEOUT,
                    self.client.request(request),
                )
                .await
                {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => {
                        evidence = Some(RuntimeFailureEvidence::ConnectFailed);
                        return Err(runtime_unavailable());
                    }
                    Err(_) => {
                        evidence = Some(RuntimeFailureEvidence::ResponseHeaderTimeout);
                        return Err(runtime_unavailable());
                    }
                };
                if response.status() != StatusCode::NO_CONTENT {
                    evidence = Some(RuntimeFailureEvidence::MalformedInternalResponse);
                    return Err(runtime_unavailable());
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            // Deletion is already authoritative and admission is fenced. An unknown cleanup
            // result must not retain disposable native namespaces indefinitely. Report
            // generation-fenced suspicion; the supervisor confirms with a functional probe
            // before any restart.
            let supervisor = self
                .supervisor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(supervisor) = supervisor {
                let _ = self.auth.with_current(&credential, || {
                    if let Some(startup_id) = endpoint.startup_id {
                        supervisor.suspect_unhealthy(
                            startup_id,
                            evidence.unwrap_or(RuntimeFailureEvidence::ConnectFailed),
                        );
                    }
                });
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "worker_loader_tests.rs"]
mod tests;
