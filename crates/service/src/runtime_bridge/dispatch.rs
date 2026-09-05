//! Bounded streaming dispatch into the current stock-workerd generation.

use super::*;

impl WorkerdTransport {
    pub(super) async fn send(
        &self,
        target: DispatchTarget,
        request: Request,
        validation: bool,
        durable_object_class: bool,
    ) -> Result<Response, PlatformError> {
        let (port, credential) = self.endpoint()?;
        let (parts, body) = request.into_parts();
        if body.size_hint().lower() > self.max_request_body as u64
            || parts
                .headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|length| length > self.max_request_body as u64)
        {
            return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
        }
        // A tenant may return without consuming its streaming body. The pinned
        // workerd can then close this HTTP/1 hop after the response, racing reuse.
        // Pool only bodyless dispatches; never buffer input or retry a mutation.
        let client = if body.is_end_stream() {
            &self.client
        } else {
            &self.body_client
        };
        let original_method = parts.method.as_str().to_owned();
        let original_url = if validation {
            "https://validation.invalid/".to_owned()
        } else {
            original_url(&parts.headers, &parts.uri)?
        };
        let mut headers = sanitize_tenant_headers(parts.headers);
        insert_header(&mut headers, TOKEN_HEADER, credential.expose())?;
        insert_header(
            &mut headers,
            "x-open-compute-account-id",
            &target.account_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-worker-id",
            &target.worker_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-version-id",
            &target.version_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-loader-key",
            &target.loader_key(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-worker-code-sha256",
            &target.worker_code_sha256,
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-route-generation",
            &target.route_generation.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-request-id",
            &target.request_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-original-method",
            &original_method,
        )?;
        insert_header(&mut headers, "x-open-compute-original-url", &original_url)?;
        if let Some(entrypoint) = &target.entrypoint {
            insert_header(&mut headers, "x-open-compute-entrypoint", entrypoint)?;
        }
        let uri: Uri = format!(
            "http://127.0.0.1:{port}{}",
            if durable_object_class {
                "/internal/validate-do"
            } else if validation {
                "/internal/validate"
            } else {
                "/internal/dispatch"
            }
        )
        .parse()
        .map_err(|_| runtime_unavailable())?;
        let mut internal =
            hyper::Request::new(Body::new(Limited::new(body, self.max_request_body)));
        *internal.method_mut() = Method::POST;
        *internal.uri_mut() = uri;
        *internal.headers_mut() = headers;
        let response =
            match tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, client.request(internal)).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) if request_body_limit_error(&error) => {
                    return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
                }
                Ok(Err(_)) | Err(_) => return Err(runtime_unavailable()),
            };
        let (mut parts, body) = response.into_parts();
        let execution_started = parts
            .headers
            .get("x-open-compute-execution-started")
            .is_some_and(|value| value == "1");
        let loader_outcome = parts
            .headers
            .get("x-open-compute-loader-outcome")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| match value {
                "cold" => Some(LoaderOutcome::Cold),
                "warm" => Some(LoaderOutcome::Warm),
                _ => None,
            });
        let asset_representation_length = if original_method == Method::HEAD.as_str() {
            parts
                .headers
                .get("x-open-compute-asset-representation-length")
                .cloned()
        } else {
            None
        };
        if execution_started && let Some(pins) = &self.version_pins {
            pins.retain_until_restart(target.version_id)?;
        }
        sanitize_response_headers(&mut parts.headers);
        if let Some(length) = asset_representation_length {
            parts.headers.insert(header::CONTENT_LENGTH, length);
        }
        if let Some(outcome) = loader_outcome {
            parts.extensions.insert(outcome);
        }
        Ok(Response::from_parts(parts, Body::new(body)))
    }
}

pub(super) fn request_body_limit_error(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        let Some(source) = error.source() else {
            return false;
        };
        error = source;
    }
}
