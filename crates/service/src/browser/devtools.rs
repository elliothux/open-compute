//! Native `DevTools` HTTP operations share the session authority and command deadline.

use super::*;
use axum::http::Method;

impl BrowserService {
    pub(super) async fn frontend(
        &self,
        id: &str,
        asset: &str,
    ) -> Result<(Vec<u8>, String), PlatformError> {
        let session = self.session(id)?;
        if let Some(managed) = &session.managed {
            let (bytes, media) = managed
                .frontend()?
                .asset(asset, self.config.max_result_bytes as usize)?;
            Ok((bytes.to_owned(), media.to_owned()))
        } else {
            let transport = self
                .frontend_transport
                .as_ref()
                .ok_or_else(backend::unavailable)?;
            BrowserBackend::frontend(&self.config, transport, asset).await
        }
    }
    pub(crate) async fn page(
        self: &Arc<Self>,
        id: &str,
        target: &str,
        request: axum::extract::Request,
    ) -> Result<axum::response::Response, PlatformError> {
        if !valid_target(target) || request.uri().query().is_some() {
            return Err(invalid());
        }
        websocket::upgrade_target(self.clone(), id, Some(target), request).await
    }
    pub(crate) async fn devtools(
        &self,
        id: &str,
        operation: &str,
        method: &Method,
        query: &BTreeMap<String, String>,
    ) -> Result<Value, PlatformError> {
        let _capacity = self
            .connections
            .clone()
            .try_acquire_owned()
            .map_err(|_| limit())?;
        let session = self.session(id)?;
        if query
            .keys()
            .any(|key| operation != "json/new" || key != "url")
        {
            return Err(http::unsupported());
        }
        let call = |command: &str, params: Value| {
            let cdp = &session.cdp;
            let request = json!({"id":0,"method":command,"params":params});
            async move {
                let reply = self.command(id, cdp, request, None).await?;
                if reply.get("error").is_some() {
                    return Err(not_found());
                }
                reply
                    .get("result")
                    .cloned()
                    .ok_or_else(backend::unavailable)
            }
        };
        match (method, operation) {
            (&Method::GET, "json/protocol") => {
                call("Browser.getVersion", json!({})).await?;
                session.inflight.fetch_add(1, Ordering::AcqRel);
                let _lease = CommandLease(session.clone());
                let value = if let Some(managed) = &session.managed {
                    let protocol = managed.frontend()?.protocol();
                    let bytes = serde_json::to_vec(protocol).map_err(|_| backend::unavailable())?;
                    if bytes.len() > self.config.max_result_bytes as usize {
                        return Err(limit());
                    }
                    protocol.clone()
                } else {
                    BrowserBackend::protocol(&self.config).await?
                };
                self.session(id)?;
                if !session.cdp.is_alive() {
                    return Err(backend::unavailable());
                }
                Ok(value)
            }
            (&Method::GET, "json/version") => {
                let value = call("Browser.getVersion", json!({})).await?;
                let string = |key| {
                    value
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or_else(backend::unavailable)
                };
                Ok(json!({
                    "Browser":string("product")?,"Protocol-Version":string("protocolVersion")?,
                    "User-Agent":string("userAgent")?,"V8-Version":string("jsVersion")?,
                    // Chromium's DevTools HTTP contract uses this fixed engine prefix.
                    "WebKit-Version":format!("537.36 ({})", string("revision")?)
                }))
            }
            (&Method::GET, "json" | "json/list") => {
                let value = call("Target.getTargets", json!({})).await?;
                value
                    .get("targetInfos")
                    .and_then(Value::as_array)
                    .ok_or_else(backend::unavailable)?
                    .iter()
                    .map(target)
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::from)
            }
            (&Method::PUT, "json/new") => {
                let url = query.get("url").map_or("about:blank", String::as_str);
                if url.len() > 4096 || url.chars().any(char::is_control) {
                    return Err(invalid());
                }
                let created = call("Target.createTarget", json!({"url":url})).await?;
                let id = created
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(backend::unavailable)?;
                let value = call("Target.getTargetInfo", json!({"targetId":id})).await?;
                target(value.get("targetInfo").ok_or_else(backend::unavailable)?)
            }
            (&Method::GET, operation) => {
                let (action, target_id) =
                    operation.rsplit_once('/').ok_or_else(http::unsupported)?;
                if !valid_target(target_id) {
                    return Err(not_found());
                }
                match action {
                    "json/list" => {
                        let value =
                            call("Target.getTargetInfo", json!({"targetId":target_id})).await?;
                        target(value.get("targetInfo").ok_or_else(backend::unavailable)?)
                    }
                    "json/activate" | "json/close" => {
                        let command = if action == "json/activate" {
                            "Target.activateTarget"
                        } else {
                            "Target.closeTarget"
                        };
                        let value = call(command, json!({"targetId":target_id})).await?;
                        if action == "json/close"
                            && value.get("success") != Some(&Value::Bool(true))
                        {
                            return Err(not_found());
                        }
                        Ok(
                            json!({"message":if action == "json/activate" {"Target activated"} else {"Target is closing"}}),
                        )
                    }
                    _ => Err(http::unsupported()),
                }
            }
            _ => Err(http::unsupported()),
        }
    }
}

fn target(info: &Value) -> Result<Value, PlatformError> {
    let text = |key| {
        info.get(key)
            .and_then(Value::as_str)
            .ok_or_else(backend::unavailable)
    };
    Ok(
        json!({"id":text("targetId")?,"type":text("type")?,"url":text("url")?,"title":text("title")?}),
    )
}

pub(super) fn valid_target(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
