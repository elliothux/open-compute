//! Bounded streaming dispatch into the current stock-workerd generation.

use super::*;

impl WorkerdTransport {
    pub(super) async fn send(
        &self,
        target: DispatchTarget,
        mut request: Request,
        validation: bool,
        durable_object_class: bool,
        probe: bool,
    ) -> Result<Response, PlatformError> {
        let endpoint = self.endpoint()?;
        let websocket = match websocket::WebSocketHandshake::capture(&mut request) {
            Ok(value) => value,
            Err(status) => return Ok(status.into_response()),
        };
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
        let mut execution_pin = if validation {
            None
        } else {
            self.version_pins
                .as_ref()
                .map(|pins| pins.pin(target.version_id))
                .transpose()?
        };
        let original_method = parts.method.as_str().to_owned();
        let original_url = if validation {
            "https://validation.invalid/".to_owned()
        } else {
            original_url(
                &parts.headers,
                &parts.uri,
                parts.extensions.get::<TrustedHttpsOrigin>().is_some(),
            )?
        };
        let mut headers = sanitize_tenant_headers(parts.headers);
        if websocket.is_some() {
            headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        }
        insert_header(
            &mut headers,
            "x-open-compute-instance-id",
            &target.instance_id.to_string(),
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
            "http://runtime{}",
            if durable_object_class {
                "/internal/validate-do"
            } else if probe {
                "/internal/probe"
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
        *internal.method_mut() = if websocket.is_some() {
            Method::GET
        } else {
            Method::POST
        };
        *internal.uri_mut() = uri;
        *internal.headers_mut() = headers;
        let mut response = match tokio::time::timeout(
            RESPONSE_HEADER_TIMEOUT,
            self.request(&endpoint.socket_path, internal),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) if request_body_limit_error(error.as_ref()) => {
                return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
            }
            Ok(Err(_)) | Err(_) => return Err(runtime_unavailable()),
        };
        let upgraded = response.status() == StatusCode::SWITCHING_PROTOCOLS;
        let service_websocket_handoffs = service_websocket_handoff_handles(response.headers())?;
        if !upgraded && !service_websocket_handoffs.is_empty() {
            return Err(runtime_unavailable());
        }
        let service_websocket_lease = if service_websocket_handoffs.is_empty() {
            None
        } else {
            Some(
                self.service_invocations
                    .as_ref()
                    .ok_or_else(runtime_unavailable)?
                    .activate_websocket_handoffs(&service_websocket_handoffs)?,
            )
        };
        if upgraded {
            websocket.ok_or_else(runtime_unavailable)?.connect(
                &mut response,
                service_websocket_lease,
                execution_pin.take(),
            )?;
        }
        let (mut parts, body) = response.into_parts();
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
        sanitize_response_headers(&mut parts.headers);
        if upgraded {
            parts
                .headers
                .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            parts
                .headers
                .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        }
        if let Some(length) = asset_representation_length {
            parts.headers.insert(header::CONTENT_LENGTH, length);
        }
        if let Some(outcome) = loader_outcome {
            parts.extensions.insert(outcome);
        }
        let response = Response::from_parts(parts, Body::new(body));
        Ok(match execution_pin {
            Some(pin) => crate::asset_backend::pin_response(response, pin),
            None => response,
        })
    }
}

fn service_websocket_handoff_handles(headers: &HeaderMap) -> Result<Vec<String>, PlatformError> {
    let values = headers
        .get_all(SERVICE_WEBSOCKET_HANDOFF_HEADER)
        .iter()
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(Vec::new());
    }
    if values.len() != 1 {
        return Err(runtime_unavailable());
    }
    let handles = values[0]
        .to_str()
        .map_err(|_| runtime_unavailable())?
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if handles.is_empty() || handles.len() > 16 {
        return Err(runtime_unavailable());
    }
    let mut unique = HashSet::with_capacity(handles.len());
    for handle in &handles {
        let parsed = uuid::Uuid::parse_str(handle).map_err(|_| runtime_unavailable())?;
        if parsed.get_version_num() != 7
            || parsed.to_string() != *handle
            || !unique.insert(handle.as_str())
        {
            return Err(runtime_unavailable());
        }
    }
    Ok(handles)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_websocket_handoff_header_is_strict_and_bounded() {
        let first = uuid::Uuid::now_v7().to_string();
        let second = uuid::Uuid::now_v7().to_string();
        let mut headers = HeaderMap::new();
        assert!(
            service_websocket_handoff_handles(&headers)
                .unwrap()
                .is_empty()
        );
        headers.insert(
            SERVICE_WEBSOCKET_HANDOFF_HEADER,
            HeaderValue::from_str(&format!("{first},{second}")).unwrap(),
        );
        assert_eq!(
            service_websocket_handoff_handles(&headers).unwrap(),
            vec![first.clone(), second]
        );

        for invalid in [
            first.to_uppercase(),
            format!("{first},{first}"),
            String::new(),
        ] {
            headers.insert(
                SERVICE_WEBSOCKET_HANDOFF_HEADER,
                HeaderValue::from_str(&invalid).unwrap(),
            );
            assert!(service_websocket_handoff_handles(&headers).is_err());
        }
    }
}
