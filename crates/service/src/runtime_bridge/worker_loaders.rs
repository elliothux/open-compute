//! Script-scoped native loader cleanup after persisted deletion and invocation drain.

use super::*;

impl WorkerdTransport {
    /// Revoke every native loader namespace owned by a deleted Script.
    pub async fn revoke_worker_loaders(&self, namespaces: &[String]) -> Result<(), PlatformError> {
        if namespaces.is_empty() {
            return Ok(());
        }
        let (port, credential) = self.endpoint()?;
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
                let response =
                    tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, self.client.request(request))
                        .await
                        .map_err(|_| runtime_unavailable())?
                        .map_err(|_| runtime_unavailable())?;
                if response.status() != StatusCode::NO_CONTENT {
                    return Err(runtime_unavailable());
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            // Deletion is already authoritative and admission is fenced. An unknown cleanup
            // result must not retain disposable native namespaces indefinitely. Let the existing
            // process owner reclaim them through its normal bounded restart path.
            let supervisor = self
                .supervisor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(supervisor) = supervisor {
                let _ = self
                    .auth
                    .with_current(&credential, || supervisor.report_unhealthy());
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "worker_loader_tests.rs"]
mod tests;
