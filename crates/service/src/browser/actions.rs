//! Quick Actions own bounded native sessions and release them on success, failure or cancellation.

use super::*;
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::Method;
use axum::response::{IntoResponse, Response};

pub(super) fn is_action(name: &str) -> bool {
    matches!(
        name,
        "content"
            | "screenshot"
            | "pdf"
            | "scrape"
            | "links"
            | "snapshot"
            | "markdown"
            | "json"
            | "accessibilityTree"
    )
}

impl BrowserService {
    pub(crate) async fn action(
        self: &Arc<Self>,
        action: &str,
        options: Value,
    ) -> Result<Response, PlatformError> {
        let observation = BrowserOperation::action(action)
            .map(|operation| self.metrics.browser_operation(operation));
        let result = async {
            if !is_action(action) || !options.is_object() {
                return Err(invalid());
            }
            if action == "json" {
                json::validate_options(&options)?;
                if options.get("custom_ai").is_none() && self.ai.default_generation_model.is_none() {
                    return Err(backend::unavailable());
                }
            }
            let transport = self.transport.as_ref().ok_or_else(backend::unavailable)?;
            let capacity = self
                .action_capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| limit())?;
            let deadline = self.config.command_timeout_ms;
            tokio::time::timeout(Duration::from_millis(deadline), async {
                // The action Worker owns the schema. Validate before allocating native browser state.
                let validation = transport.browser_action(json!({
                    "sessionId":uuid::Uuid::now_v7().to_string(), "validateOnly":true,
                    "action":action, "options":options, "maxResultBytes":self.config.max_result_bytes
                }), self.config.max_result_bytes as usize).await?;
                if validation.status() != axum::http::StatusCode::NO_CONTENT {
                    return Ok(validation);
                }
                let id = self.acquire(60_000).await?;
                let lease = ActionLease { service: self.clone(), id, _capacity: capacity, command: None };
                let session = self.session(&lease.id)?;
                session.inflight.fetch_add(1, Ordering::AcqRel);
                session.is_action.store(true, Ordering::Release);
                let mut lease = lease;
                lease.command = Some(CommandLease(session));
                self.active_actions.lock().map_err(|_| backend::unavailable())?.insert(lease.id.clone());
                transport.browser_action(json!({"sessionId":lease.id,"action":action,"options":options,"maxResultBytes":self.config.max_result_bytes}), self.config.max_result_bytes as usize).await
            }).await.map_err(|_| timeout())?
        }.await;
        if let Some(observation) = observation {
            let outcome = match &result {
                Ok(response) => BrowserOutcome::status(response.status()),
                Err(_) => BrowserOutcome::result(&result),
            };
            observation.finish(outcome);
        }
        result
    }

    pub(crate) async fn handle_action_backend(
        self: &Arc<Self>,
        request: Request,
        parser: Option<&crate::document_parser_backend::DocumentParserBindingService>,
    ) -> Response {
        self.action_backend(request, parser)
            .await
            .unwrap_or_else(|error| http::error(&error))
    }

    async fn action_backend(
        self: &Arc<Self>,
        request: Request,
        parser: Option<&crate::document_parser_backend::DocumentParserBindingService>,
    ) -> Result<Response, PlatformError> {
        let path = request
            .uri()
            .path()
            .strip_prefix("/internal/browser-actions/v1/")
            .ok_or_else(not_found)?
            .to_owned();
        let (id, operation) = path
            .rsplit_once('/')
            .map_or((path.as_str(), ""), |(id, operation)| (id, operation));
        if request.uri().query().is_some()
            || !self
                .active_actions
                .lock()
                .map_err(|_| backend::unavailable())?
                .contains(id)
        {
            return Err(not_found());
        }
        self.session(id)?;
        if operation == "json" && request.method() == Method::POST {
            return self
                .extract_json(request, parser.ok_or_else(backend::unavailable)?)
                .await;
        }
        if operation == "markdown" && request.method() == Method::POST {
            let parser = parser.ok_or_else(backend::unavailable)?;
            let bytes = to_bytes(request.into_body(), self.config.max_body_bytes as usize)
                .await
                .map_err(|_| limit())?;
            let parsed = parser
                .parse_for_ai_search("page.html", "text/html", bytes.to_vec())
                .await
                .map_err(|_| backend::unavailable())?;
            if parsed.markdown.len() > self.config.max_result_bytes as usize {
                return Err(limit());
            }
            return Ok(axum::Json(parsed.markdown).into_response());
        }
        if operation.is_empty() && request.method() == Method::GET {
            return websocket::upgrade(self.clone(), id, request).await;
        }
        Err(http::unsupported())
    }
}

struct ActionLease {
    service: Arc<BrowserService>,
    id: String,
    _capacity: OwnedSemaphorePermit,
    command: Option<CommandLease>,
}
impl Drop for ActionLease {
    fn drop(&mut self) {
        if let Ok(mut actions) = self.service.active_actions.lock() {
            actions.remove(&self.id);
        }
        let _ = self
            .service
            .close_now(&self.id, BrowserSessionCloseReason::Normal);
    }
}
