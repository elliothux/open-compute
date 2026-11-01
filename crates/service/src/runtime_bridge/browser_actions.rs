//! Bounded calls into the isolated platform-owned Puppeteer action Worker.

use super::*;

impl WorkerdTransport {
    pub(crate) async fn browser_action(
        &self,
        body: serde_json::Value,
        maximum: usize,
    ) -> Result<Response, PlatformError> {
        let endpoint = self.endpoint()?;
        let request = hyper::Request::builder()
            .method(Method::POST)
            .uri("http://runtime/internal/browser-action")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&body).map_err(|_| crate::browser::invalid())?,
            ))
            .map_err(|_| crate::browser::invalid())?;
        let response = self
            .request(&endpoint.socket_path, request)
            .await
            .map_err(|_| runtime_unavailable())?;
        let status = response.status();
        let mut headers = HeaderMap::new();
        for name in ["content-type", "x-browser-ms-used"] {
            if let Some(value) = response.headers().get(name) {
                headers.insert(HeaderName::from_static(name), value.clone());
            }
        }
        let bytes = to_bytes(Body::new(response.into_body()), maximum)
            .await
            .map_err(|_| crate::browser::limit())?;
        let mut response = (status, bytes).into_response();
        *response.headers_mut() = headers;
        Ok(response)
    }
}
