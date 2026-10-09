//! Managed CDP authorization: one session owns contexts, targets, and flattened attachments.

use open_compute_core::{ErrorCode, PlatformError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[path = "scope_frames.rs"]
mod frames;
#[path = "scope_policy.rs"]
mod policy;
#[path = "scope_windows.rs"]
mod windows;
use policy::{page_event, page_method};

#[derive(Debug)]
struct Namespace {
    default_context: String,
    contexts: BTreeMap<String, String>,
    targets: BTreeMap<String, String>,
    frames: BTreeMap<String, String>,
    downloads: BTreeMap<String, String>,
    download_limit: usize,
    download_overflow: bool,
}

#[derive(Debug)]
struct Attachment {
    engine: String,
    target: String,
}

/// Session-local namespace, never a raw shared-browser endpoint.
#[derive(Debug)]
pub struct BrowserScope {
    namespace: Arc<Mutex<Namespace>>,
    attachments: BTreeMap<String, Attachment>,
    streams: BTreeMap<String, String>,
    windows: BTreeMap<u64, windows::Window>,
}

impl BrowserScope {
    /// Own a newly allocated default context with a bounded download namespace; native locators stay private.
    #[must_use]
    pub fn new(default_context: String, download_limit: usize) -> Self {
        Self {
            namespace: Arc::new(Mutex::new(Namespace {
                default_context,
                contexts: BTreeMap::new(),
                targets: BTreeMap::new(),
                frames: BTreeMap::new(),
                downloads: BTreeMap::new(),
                download_limit,
                download_overflow: false,
            })),
            attachments: BTreeMap::new(),
            streams: BTreeMap::new(),
            windows: BTreeMap::new(),
        }
    }

    /// Current owned physical context locators, including the hidden default context.
    pub fn engine_contexts(&self) -> Result<Vec<String>, PlatformError> {
        let namespace = self.namespace.lock().map_err(|_| denied())?;
        Ok(std::iter::once(namespace.default_context.clone())
            .chain(namespace.contexts.values().cloned())
            .collect())
    }

    pub(super) fn download_overflow(&self) -> bool {
        self.namespace
            .lock()
            .map_or(true, |namespace| namespace.download_overflow)
    }

    /// Additional context aliases; the physical default context is never published.
    pub fn context_ids(&self) -> Result<Vec<String>, PlatformError> {
        Ok(self
            .namespace
            .lock()
            .map_err(|_| denied())?
            .contexts
            .keys()
            .cloned()
            .collect())
    }

    /// Share session context/target identity while giving a client independent attachments/streams.
    #[must_use]
    pub fn fork(&self) -> Self {
        Self {
            namespace: self.namespace.clone(),
            attachments: BTreeMap::new(),
            streams: BTreeMap::new(),
            windows: BTreeMap::new(),
        }
    }

    /// Admit an engine target only after proving its `BrowserContext` belongs to this session.
    pub fn target_info(&mut self, value: &Value) -> Option<Value> {
        let context = value.get("browserContextId")?.as_str()?;
        let mut namespace = self.namespace.lock().ok()?;
        if namespace.default_context != context
            && !namespace.contexts.values().any(|owned| owned == context)
        {
            return None;
        }
        let engine = value.get("targetId")?.as_str()?;
        let target = frames::target(&mut namespace, engine);
        let mut info = value.as_object()?.clone();
        info.insert("targetId".into(), target.into());
        if context == namespace.default_context {
            info.remove("browserContextId");
        } else {
            let context = namespace
                .contexts
                .iter()
                .find(|(_, raw)| raw.as_str() == context)?
                .0;
            info.insert("browserContextId".into(), context.clone().into());
        }
        // Browser frontend URLs and opener identifiers must never expose another target.
        info.remove("devtoolsFrontendUrl");
        for key in ["openerId", "openerFrameId"] {
            info.remove(key);
        }
        Some(Value::Object(info))
    }

    /// Publish an explicitly created additional context under an opaque session identifier.
    pub fn add_context(&mut self, engine: &str) -> Result<String, PlatformError> {
        Ok(alias(
            &mut self.namespace.lock().map_err(|_| denied())?.contexts,
            engine,
        ))
    }

    fn add_attachment(&mut self, engine: &str, target: &str) -> Result<String, PlatformError> {
        if let Some((public, attachment)) = self
            .attachments
            .iter()
            .find(|(_, item)| item.engine == engine)
        {
            if attachment.target != target {
                return Err(denied());
            }
            return Ok(public.clone());
        }
        let public = uuid::Uuid::now_v7().to_string();
        self.attachments.insert(
            public.clone(),
            Attachment {
                engine: engine.to_owned(),
                target: target.to_owned(),
            },
        );
        Ok(public)
    }

    /// Physical attachments retained by this connection for detach cleanup.
    #[must_use]
    pub fn engine_attachments(&self) -> Vec<String> {
        self.attachments
            .values()
            .map(|item| item.engine.clone())
            .collect()
    }

    /// Remove a successfully disposed additional context; the hidden default cannot be disposed.
    pub fn remove_context(&mut self, public: &str) -> Result<(), PlatformError> {
        self.namespace
            .lock()
            .map_err(|_| denied())?
            .contexts
            .remove(public);
        Ok(())
    }

    /// Return an owned physical target locator for a scoped HTTP/page operation.
    pub fn target(&self, public: &str) -> Result<String, PlatformError> {
        self.namespace
            .lock()
            .map_err(|_| denied())?
            .targets
            .get(public)
            .cloned()
            .ok_or_else(denied)
    }

    pub(super) fn window_target(&self, params: &Value) -> Result<String, PlatformError> {
        let id = params
            .get("windowId")
            .and_then(Value::as_u64)
            .ok_or_else(denied)?;
        let window = self.windows.get(&id).ok_or_else(denied)?;
        self.target(&window.target)
    }

    /// Validate and translate a qualified command before touching the physical browser.
    /// Browser.close, context enumeration and discovery/auto-attach are manager-owned operations.
    pub fn command(
        &self,
        method: &str,
        mut params: Value,
        session: Option<&str>,
    ) -> Result<(Value, Option<String>), PlatformError> {
        let fields = params.as_object_mut().ok_or_else(denied)?;
        let attachment = session
            .map(|id| {
                self.attachments
                    .get(id)
                    .map(|item| item.engine.clone())
                    .ok_or_else(denied)
            })
            .transpose()?;
        let namespace = self.namespace.lock().map_err(|_| denied())?;
        match method {
            "Browser.getWindowForTarget"
            | "Browser.getWindowBounds"
            | "Browser.setWindowBounds"
                if attachment.is_some() =>
            {
                windows::command(method, fields, &self.windows, &namespace.targets)?;
            }
            "Browser.getVersion" => {
                require_fields(fields, &[])?;
            }
            "Target.createBrowserContext" => {
                require_fields(fields, &["disposeOnDetach"])?;
                if fields
                    .get("disposeOnDetach")
                    .is_some_and(|v| !v.is_boolean())
                {
                    return Err(denied());
                }
            }
            "Target.createTarget" => {
                require_fields(
                    fields,
                    &[
                        "url",
                        "browserContextId",
                        "width",
                        "height",
                        "background",
                        "newWindow",
                        "forTab",
                    ],
                )?;
                if !fields.get("url").is_some_and(Value::is_string) {
                    return Err(denied());
                }
                validate_navigation(fields.get("url").and_then(Value::as_str))?;
                context_field(fields, &namespace.default_context, &namespace.contexts)?;
            }
            "Target.disposeBrowserContext" => {
                require_fields(fields, &["browserContextId"])?;
                let public = fields
                    .get("browserContextId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let engine = namespace.contexts.get(public).ok_or_else(denied)?;
                fields.insert("browserContextId".into(), engine.clone().into());
            }
            "Target.attachToTarget"
            | "Target.closeTarget"
            | "Target.activateTarget"
            | "Target.getTargetInfo" => {
                require_fields(fields, &["targetId", "flatten"])?;
                if method == "Target.getTargetInfo" && fields.is_empty() && attachment.is_some() {
                    return Ok((params, attachment));
                }
                if method == "Target.attachToTarget"
                    && fields.get("flatten") != Some(&Value::Bool(true))
                {
                    return Err(denied());
                }
                let public = fields
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let engine = namespace.targets.get(public).ok_or_else(denied)?;
                fields.insert("targetId".into(), engine.clone().into());
            }
            "Target.detachFromTarget" => {
                require_fields(fields, &["sessionId"])?;
                let public = fields
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let engine = self.attachments.get(public).ok_or_else(denied)?;
                fields.insert("sessionId".into(), engine.engine.clone().into());
            }
            "Target.setAutoAttach" if attachment.is_some() => {
                require_fields(
                    fields,
                    &["autoAttach", "waitForDebuggerOnStart", "flatten", "filter"],
                )?;
                if fields.get("flatten") != Some(&Value::Bool(true))
                    || !fields.get("autoAttach").is_some_and(Value::is_boolean)
                    || !fields
                        .get("waitForDebuggerOnStart")
                        .is_some_and(Value::is_boolean)
                {
                    return Err(unsupported());
                }
            }
            "IO.read" | "IO.close" => {
                require_fields(fields, &["handle", "offset", "size"])?;
                let public = fields
                    .get("handle")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let engine = self.streams.get(public).ok_or_else(denied)?;
                fields.insert("handle".into(), engine.clone().into());
            }
            "Storage.getCookies" | "Storage.setCookies" | "Storage.clearCookies" => {
                require_fields(fields, &["browserContextId", "cookies"])?;
                context_field(fields, &namespace.default_context, &namespace.contexts)?;
            }
            "Browser.setDownloadBehavior" => {
                require_fields(
                    fields,
                    &[
                        "behavior",
                        "browserContextId",
                        "eventsEnabled",
                        "downloadPath",
                    ],
                )?;
                if !matches!(
                    fields.get("behavior").and_then(Value::as_str),
                    Some("deny" | "allowAndName")
                ) || fields
                    .get("eventsEnabled")
                    .is_some_and(|value| !value.is_boolean())
                    || fields.get("downloadPath").is_some_and(|value| {
                        value.as_str().is_none_or(|path| {
                            path.is_empty()
                                || path.len() > 4_096
                                || path.bytes().any(|byte| byte.is_ascii_control())
                        })
                    })
                    || (fields.get("behavior") == Some(&json!("allowAndName"))
                        && !fields.contains_key("downloadPath"))
                {
                    return Err(denied());
                }
                context_field(fields, &namespace.default_context, &namespace.contexts)?;
            }
            "Browser.cancelDownload" => {
                require_fields(fields, &["guid", "browserContextId"])?;
                let public = fields
                    .get("guid")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let engine = namespace.downloads.get(public).ok_or_else(denied)?;
                fields.insert("guid".into(), engine.clone().into());
                context_field(fields, &namespace.default_context, &namespace.contexts)?;
            }
            _ => {
                if attachment.is_none()
                    || !page_method(method)
                    || fields.keys().any(|key| {
                        matches!(
                            key.as_str(),
                            "browserContextId" | "targetId" | "sessionId" | "downloadPath" | "path"
                        )
                    })
                {
                    return Err(unsupported());
                }
                if method == "Page.navigate"
                    || (method == "Fetch.continueRequest" && fields.contains_key("url"))
                {
                    validate_navigation(fields.get("url").and_then(Value::as_str))?;
                }
                frames::command(method, fields, &namespace)?;
            }
        }
        if attachment.is_some()
            && method.starts_with("Browser.")
            && !matches!(
                method,
                "Browser.getWindowForTarget"
                    | "Browser.getWindowBounds"
                    | "Browser.setWindowBounds"
            )
        {
            return Err(unsupported());
        }
        Ok((params, attachment))
    }

    /// Translate only protocol-owned result fields; evaluated application objects remain intact.
    pub fn reply(
        &mut self,
        method: &str,
        original: &Value,
        response: Value,
    ) -> Result<Value, PlatformError> {
        let Value::Object(mut response) = response else {
            return Err(denied());
        };
        if response.contains_key("error") {
            return Ok(json!({"error":{"code":-32000,"message":"Browser command failed"}}));
        }
        let Some(Value::Object(mut result)) = response.remove("result") else {
            return Err(denied());
        };
        match method {
            "Browser.getWindowForTarget" => {
                windows::reply(original, &mut result, &mut self.windows)?;
            }
            "Target.createBrowserContext" => {
                let raw = result
                    .get("browserContextId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let public = self.add_context(raw)?;
                result.insert("browserContextId".into(), public.into());
            }
            "Target.createTarget" => {
                let raw = result
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let mut namespace = self.namespace.lock().map_err(|_| denied())?;
                let public = frames::target(&mut namespace, raw);
                result.insert("targetId".into(), public.into());
            }
            "Target.attachToTarget" => {
                let raw = result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let target = original
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(denied)?;
                let public = self.add_attachment(raw, target)?;
                result.insert("sessionId".into(), public.into());
            }
            "Target.getTargetInfo" => {
                let info = self
                    .target_info(result.get("targetInfo").ok_or_else(denied)?)
                    .ok_or_else(denied)?;
                result.insert("targetInfo".into(), info);
            }
            "Target.getTargets" => {
                let infos = result
                    .get("targetInfos")
                    .and_then(Value::as_array)
                    .ok_or_else(denied)?;
                let infos = infos
                    .iter()
                    .filter_map(|info| self.target_info(info))
                    .collect::<Vec<_>>();
                result.insert("targetInfos".into(), infos.into());
            }
            "Target.disposeBrowserContext" => {
                self.remove_context(
                    original
                        .get("browserContextId")
                        .and_then(Value::as_str)
                        .ok_or_else(denied)?,
                )?;
            }
            "Page.printToPDF" => {
                if let Some(raw) = result.get("stream").and_then(Value::as_str) {
                    let public = alias(&mut self.streams, raw);
                    result.insert("stream".into(), public.into());
                }
            }
            "IO.close" => {
                self.streams.remove(
                    original
                        .get("handle")
                        .and_then(Value::as_str)
                        .ok_or_else(denied)?,
                );
            }
            "Target.detachFromTarget" => {
                self.attachments.remove(
                    original
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .ok_or_else(denied)?,
                );
            }
            _ => {}
        }
        let mut namespace = self.namespace.lock().map_err(|_| denied())?;
        frames::reply(method, &mut result, &mut namespace)?;
        Ok(json!({"result":result}))
    }

    /// Filter and translate engine events; unknown browser-global events never escape.
    pub fn event(&mut self, value: &Value) -> Option<Value> {
        let method = value.get("method")?.as_str()?;
        let mut event = value.as_object()?.clone();
        if let Some(raw) = value.get("sessionId").and_then(Value::as_str) {
            let public = self
                .attachments
                .iter()
                .find(|(_, item)| item.engine == raw)?
                .0
                .clone();
            if method == "Target.attachedToTarget" {
                let params = value.get("params")?;
                let target = self.target_info(params.get("targetInfo")?)?;
                let child = self
                    .add_attachment(
                        params.get("sessionId")?.as_str()?,
                        target.get("targetId")?.as_str()?,
                    )
                    .ok()?;
                event.insert("params".into(), json!({"sessionId":child,"targetInfo":target,"waitingForDebugger":params.get("waitingForDebugger")?}));
                event.insert("sessionId".into(), public.into());
                return Some(Value::Object(event));
            }
            if method == "Target.detachedFromTarget" {
                let raw = value.pointer("/params/sessionId")?.as_str()?;
                let child = self
                    .attachments
                    .iter()
                    .find(|(_, item)| item.engine == raw)?
                    .0
                    .clone();
                let attachment = self.attachments.remove(&child)?;
                event.insert(
                    "params".into(),
                    json!({"sessionId":child,"targetId":attachment.target}),
                );
                event.insert("sessionId".into(), public.into());
                return Some(Value::Object(event));
            }
            if !page_event(method) {
                return None;
            }
            let mut namespace = self.namespace.lock().ok()?;
            frames::event(method, event.get_mut("params")?, &mut namespace).ok()?;
            event.insert("sessionId".into(), public.into());
            return Some(Value::Object(event));
        }
        let params = value.get("params")?;
        match method {
            "Browser.downloadWillBegin" | "Browser.downloadProgress" => {
                let mut namespace = self.namespace.lock().ok()?;
                let engine = params.get("guid")?.as_str()?;
                let mut params = params.as_object()?.clone();
                if method == "Browser.downloadWillBegin" {
                    let frame = params.get("frameId")?.as_str()?;
                    let public = namespace
                        .frames
                        .iter()
                        .chain(namespace.targets.iter())
                        .find(|(_, raw)| raw.as_str() == frame)?
                        .0
                        .clone();
                    params.insert("frameId".into(), public.into());
                    if !namespace.downloads.values().any(|guid| guid == engine)
                        && namespace.downloads.len() >= namespace.download_limit
                    {
                        namespace.download_overflow = true;
                        return None;
                    }
                    alias(&mut namespace.downloads, engine);
                }
                let public = namespace
                    .downloads
                    .iter()
                    .find(|(_, raw)| raw.as_str() == engine)?
                    .0
                    .clone();
                params.insert("guid".into(), public.into());
                params.remove("filePath");
                event.insert("params".into(), Value::Object(params));
            }
            "Target.attachedToTarget" => {
                let target = self.target_info(params.get("targetInfo")?)?;
                let attachment = self
                    .add_attachment(
                        params.get("sessionId")?.as_str()?,
                        target.get("targetId")?.as_str()?,
                    )
                    .ok()?;
                event.insert("params".into(), json!({"sessionId":attachment,"targetInfo":target,"waitingForDebugger":params.get("waitingForDebugger")?}));
            }
            "Target.detachedFromTarget" => {
                let raw = params.get("sessionId")?.as_str()?;
                let public = self
                    .attachments
                    .iter()
                    .find(|(_, item)| item.engine == raw)?
                    .0
                    .clone();
                let attachment = self.attachments.remove(&public)?;
                event.insert(
                    "params".into(),
                    json!({"sessionId":public,"targetId":attachment.target}),
                );
            }
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                event.insert(
                    "params".into(),
                    json!({"targetInfo": self.target_info(params.get("targetInfo")?)?}),
                );
            }
            "Target.targetDestroyed" | "Target.targetCrashed" => {
                let engine = params.get("targetId")?.as_str()?;
                let namespace = self.namespace.lock().ok()?;
                let public = namespace
                    .targets
                    .iter()
                    .find(|(_, raw)| raw.as_str() == engine)?
                    .0
                    .clone();
                let mut params = params.as_object()?.clone();
                params.insert("targetId".into(), public.into());
                event.insert("params".into(), Value::Object(params));
            }
            _ => return None,
        }
        Some(Value::Object(event))
    }
}

fn alias(map: &mut BTreeMap<String, String>, engine: &str) -> String {
    if let Some((public, _)) = map.iter().find(|(_, raw)| raw.as_str() == engine) {
        return public.clone();
    }
    let public = uuid::Uuid::now_v7().to_string();
    map.insert(public.clone(), engine.to_owned());
    public
}
fn context_field(
    fields: &mut serde_json::Map<String, Value>,
    default: &str,
    contexts: &BTreeMap<String, String>,
) -> Result<(), PlatformError> {
    let engine = match fields.get("browserContextId") {
        None => default,
        Some(public) => contexts
            .get(public.as_str().ok_or_else(denied)?)
            .map(String::as_str)
            .ok_or_else(denied)?,
    };
    fields.insert("browserContextId".into(), engine.into());
    Ok(())
}
fn require_fields(
    fields: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), PlatformError> {
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(unsupported());
    }
    Ok(())
}

fn validate_navigation(value: Option<&str>) -> Result<(), PlatformError> {
    let value = value.ok_or_else(denied)?;
    let url = url::Url::parse(value).map_err(|_| denied())?;
    if !matches!(url.scheme(), "http" | "https" | "data" | "blob") && value != "about:blank" {
        return Err(unsupported());
    }
    Ok(())
}
fn denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserSessionNotFound,
        "browser target or context is unavailable",
    )
}
fn unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserUnsupported,
        "browser CDP method or field is unsupported",
    )
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
