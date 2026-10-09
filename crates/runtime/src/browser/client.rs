//! Independent managed client attachments with session-shared context/target identity.

use super::{BrowserCdp, BrowserScope, ManagedBrowserSession};
use open_compute_core::{ErrorCode, PlatformError};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, OwnedSemaphorePermit, mpsc};

struct State {
    scope: BrowserScope,
    owned_contexts: BTreeSet<String>,
    related_targets: BTreeSet<String>,
    discover: bool,
    auto_attach: Option<Value>,
    discovery_filter: Vec<Value>,
    target_attachment: Option<String>,
}

struct Client {
    owner: Arc<ManagedBrowserSession>,
    browser_attachment: String,
    capacity: Arc<OwnedSemaphorePermit>,
    state: Arc<Mutex<State>>,
    closed: AtomicBool,
}

impl ManagedBrowserSession {
    /// Create an independently owned browser/page client over the generation's private pipe.
    pub async fn connect(
        self: &Arc<Self>,
        target: Option<&str>,
    ) -> Result<BrowserCdp, PlatformError> {
        if !self.is_alive() {
            return Err(unavailable());
        }
        let capacity = Arc::new(
            self.generation
                .clients
                .clone()
                .try_acquire_owned()
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::BrowserLimitExceeded,
                        "browser client capacity exceeded",
                    )
                })?,
        );
        let owner = self.clone();
        let target = target.map(str::to_owned);
        tokio::spawn(async move { owner.connect_inner(target.as_deref(), capacity).await })
            .await
            .map_err(|_| unavailable())?
    }

    async fn connect_inner(
        self: &Arc<Self>,
        target: Option<&str>,
        capacity: Arc<OwnedSemaphorePermit>,
    ) -> Result<BrowserCdp, PlatformError> {
        let scope = self.scope.lock().await.fork();
        let mut events = self.generation.cdp().subscribe();
        let reply = self
            .generation
            .cdp()
            .command("Target.attachToBrowserTarget", json!({}), None)
            .await
            .inspect_err(|_| self.generation.cdp().invalidate())?;
        let browser_attachment = reply
            .pointer("/result/sessionId")
            .and_then(Value::as_str)
            .ok_or_else(unavailable)?
            .to_owned();
        let client = Arc::new(Client {
            owner: self.clone(),
            browser_attachment,
            capacity,
            state: Arc::new(Mutex::new(State {
                scope,
                owned_contexts: BTreeSet::new(),
                related_targets: BTreeSet::new(),
                discover: false,
                auto_attach: None,
                discovery_filter: vec![json!({})],
                target_attachment: None,
            })),
            closed: AtomicBool::new(false),
        });
        let discovery = client
            .owner
            .generation
            .cdp()
            .command(
                "Target.setDiscoverTargets",
                json!({"discover":true,"filter":[{}]}),
                Some(&client.browser_attachment),
            )
            .await?;
        if discovery.get("error").is_some() {
            return Err(unavailable());
        }
        if let Some(target) = target {
            let mut state = client.state.lock().await;
            let reply = client
                .scoped(
                    &mut state,
                    "Target.attachToTarget",
                    json!({"targetId":target,"flatten":true}),
                    None,
                )
                .await?;
            state.target_attachment = Some(
                reply
                    .pointer("/result/sessionId")
                    .and_then(Value::as_str)
                    .ok_or_else(unavailable)?
                    .to_owned(),
            );
        }
        let generation = &self.generation;
        let (outbound, mut commands) = mpsc::channel::<Value>(generation.queue);
        let (frames, inbound) = mpsc::channel(generation.queue);
        let max_message = generation.max_message;
        let deadline = generation.deadline;
        let worker = tokio::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                let output = tokio::select! {
                    command = commands.recv() => {
                        let Some(command) = command else { break; };
                        let id = command.get("id").cloned();
                        let method = command.get("method").and_then(Value::as_str).map(str::to_owned);
                        let for_tab = command.pointer("/params/forTab") == Some(&Value::Bool(true));
                        if command.to_string().len() > max_message { break; }
                        let owner = client.clone();
                        // Context/attachment replies remain owned when this bridge is cancelled.
                        let result = tokio::spawn(async move { owner.command(command).await }).await;
                        match result {
                            Ok(Ok(values)) => match client.before_reply(method.as_deref(), for_tab, values, &mut events).await {
                                Ok(values) => values,
                                Err(_) => break,
                            },
                            Ok(Err(error)) => {
                                vec![json!({"id":id,"error":{"code":-32000,"message":error.code().as_str()}})]
                            }
                            _ => vec![json!({"id":id,"error":{"code":-32000,"message":"BROWSER_UNSUPPORTED"}})],
                        }
                    }
                    event = events.recv() => {
                        let Ok(event) = event else { break; };
                        match client.event(event).await { Ok(values) => values, Err(_) => break }
                    }
                    _ = timer.tick() => {
                        if !client.owner.is_alive() { break; }
                        continue;
                    }
                };
                for value in output {
                    if value.to_string().len() > max_message
                        || !matches!(
                            tokio::time::timeout(deadline, frames.send(value)).await,
                            Ok(Ok(()))
                        )
                    {
                        return;
                    }
                }
            }
        });
        BrowserCdp::start(
            outbound,
            inbound,
            generation.queue,
            deadline,
            vec![worker.abort_handle()],
        )
    }
}

impl Client {
    async fn before_reply(
        &self,
        method: Option<&str>,
        for_tab: bool,
        replies: Vec<Value>,
        events: &mut super::BrowserCdpEvents,
    ) -> Result<Vec<Value>, PlatformError> {
        if method != Some("Target.createTarget") {
            return Ok(replies);
        }
        let Some(target) = replies
            .last()
            .and_then(|reply| reply.pointer("/result/targetId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok(replies);
        };
        let state = self.state.lock().await;
        let Some(policy) = &state.auto_attach else {
            return Ok(replies);
        };
        let filters = policy
            .get("filter")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![json!({})]);
        if !matches_filter(
            &filters,
            &json!({"type":if for_tab { "tab" } else { "page" }}),
        ) {
            return Ok(replies);
        }
        drop(state);
        // Native creation publishes the automatic attachment before its response.
        // Reproduce that ordering after the scoped related-target attachment completes.
        tokio::time::timeout(self.owner.generation.deadline, async {
            let mut output = Vec::new();
            loop {
                let event = events.recv().await.map_err(|_| unavailable())?;
                for value in self.event(event).await? {
                    let attached = value["method"] == "Target.attachedToTarget"
                        && value
                            .pointer("/params/targetInfo/targetId")
                            .and_then(Value::as_str)
                            == Some(target.as_str());
                    output.push(value);
                    if output.len() > self.owner.generation.queue {
                        return Err(unavailable());
                    }
                    if attached {
                        output.extend(replies);
                        return Ok(output);
                    }
                }
            }
        })
        .await
        .map_err(|_| unavailable())?
    }

    async fn command(&self, request: Value) -> Result<Vec<Value>, PlatformError> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(unsupported)?;
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let requested_attachment = request.get("sessionId").and_then(Value::as_str);
        let mut state = self.state.lock().await;
        if self.closed.load(Ordering::Acquire) || !self.owner.is_alive() {
            return Err(unavailable());
        }
        let page = state.target_attachment.clone();
        if page.is_some() && requested_attachment.is_some() {
            return Err(unsupported());
        }
        let attachment = page.as_deref().or(requested_attachment);
        let mut events = Vec::new();
        let mut reply = match method {
            "Browser.close" if attachment.is_none() => {
                drop(state);
                if params != json!({}) {
                    return Err(unsupported());
                }
                self.owner.close().await?;
                json!({"result":{}})
            }
            "Target.getBrowserContexts" if attachment.is_none() && params == json!({}) => {
                json!({"result":{"browserContextIds":state.scope.context_ids()?}})
            }
            "Target.getTargets" if attachment.is_none() && params == json!({}) => {
                let reply = self
                    .owner
                    .generation
                    .cdp()
                    .command(method, params.clone(), None)
                    .await?;
                state.scope.reply(method, &params, reply)?
            }
            "Target.setDiscoverTargets" if attachment.is_none() => {
                validate_policy(&params, false)?;
                state.discover = params["discover"].as_bool().ok_or_else(unsupported)?;
                state.discovery_filter = params
                    .get("filter")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| vec![json!({})]);
                if state.discover {
                    for info in self.targets(&mut state).await? {
                        if matches_filter(&state.discovery_filter, &info) {
                            events.push(json!({"method":"Target.targetCreated","params":{"targetInfo":info}}));
                        }
                    }
                }
                json!({"result":{}})
            }
            "Target.setAutoAttach" if attachment.is_none() => {
                validate_policy(&params, true)?;
                let enabled = params["autoAttach"].as_bool().ok_or_else(unsupported)?;
                let cleared = self
                    .owner
                    .generation
                    .cdp()
                    .command(
                        method,
                        json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}),
                        Some(&self.browser_attachment),
                    )
                    .await?;
                if cleared.get("error").is_some() {
                    return Err(unavailable());
                }
                state.related_targets.clear();
                state.auto_attach = enabled.then_some(params);
                for info in self.targets(&mut state).await? {
                    self.auto_attach(&mut state, &info).await?;
                }
                json!({"result":{}})
            }
            _ => self.scoped(&mut state, method, params, attachment).await?,
        };
        reply["id"] = request["id"].clone();
        if page.is_none()
            && let Some(attachment) = requested_attachment
        {
            reply["sessionId"] = attachment.into();
        }
        events.push(reply);
        Ok(events)
    }

    async fn scoped(
        &self,
        state: &mut State,
        method: &str,
        mut original: Value,
        attachment: Option<&str>,
    ) -> Result<Value, PlatformError> {
        let _context_guard = if matches!(
            method,
            "Target.createBrowserContext" | "Target.disposeBrowserContext"
        ) {
            Some(self.owner.scope.lock().await)
        } else {
            None
        };
        if !self.owner.is_alive() {
            return Err(unavailable());
        }
        let dispose_on_detach = method == "Target.createBrowserContext"
            && original.get("disposeOnDetach") == Some(&Value::Bool(true));
        let (mut params, attachment) = state.scope.command(method, original.clone(), attachment)?;
        if method == "Browser.getWindowForTarget" && original.get("targetId").is_none() {
            let info = self
                .owner
                .generation
                .cdp()
                .command("Target.getTargetInfo", json!({}), attachment.as_deref())
                .await?;
            let info = state
                .scope
                .reply("Target.getTargetInfo", &json!({}), info)?;
            let target = info
                .pointer("/result/targetInfo/targetId")
                .and_then(Value::as_str)
                .ok_or_else(unavailable)?;
            original["targetId"] = target.into();
            params["targetId"] = state.scope.target(target)?.into();
        }
        if matches!(
            method,
            "Browser.getWindowBounds" | "Browser.setWindowBounds"
        ) {
            let target = state.scope.window_target(&original)?;
            let info = self
                .owner
                .generation
                .cdp()
                .command(
                    "Target.getTargetInfo",
                    json!({"targetId":target}),
                    attachment.as_deref(),
                )
                .await?;
            let info = state
                .scope
                .reply("Target.getTargetInfo", &json!({}), info)?;
            if info.get("error").is_some() {
                return Err(unsupported());
            }
            let window = self
                .owner
                .generation
                .cdp()
                .command(
                    "Browser.getWindowForTarget",
                    json!({"targetId":target}),
                    attachment.as_deref(),
                )
                .await?;
            if window.pointer("/result/windowId") != params.get("windowId") {
                return Err(unsupported());
            }
        }
        if method == "Browser.setDownloadBehavior" {
            self.owner.downloads.configure(&mut params)?;
        }
        let disposing_context = if method == "Target.disposeBrowserContext" {
            params
                .get("browserContextId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let reply = self
            .owner
            .generation
            .cdp()
            .command(
                method,
                params,
                Some(attachment.as_deref().unwrap_or(&self.browser_attachment)),
            )
            .await
            .inspect_err(|_| {
                if method == "Target.createBrowserContext" {
                    self.owner.generation.cdp().invalidate();
                }
            })?;
        let reply = state.scope.reply(method, &original, reply)?;
        if reply.get("error").is_none()
            && let Some(context) = disposing_context
        {
            self.owner
                .downloads
                .remove_context(&context)
                .inspect_err(|_| self.owner.generation.cdp().invalidate())?;
        }
        if dispose_on_detach
            && let Some(context) = reply
                .pointer("/result/browserContextId")
                .and_then(Value::as_str)
        {
            state.owned_contexts.insert(context.to_owned());
        }
        if method == "Target.createBrowserContext" && reply.get("error").is_none() {
            let context = reply
                .pointer("/result/browserContextId")
                .and_then(Value::as_str)
                .ok_or_else(unavailable)?;
            let (params, _) = state.scope.command(
                "Browser.setDownloadBehavior",
                json!({"behavior":"deny","browserContextId":context}),
                None,
            )?;
            let denied = self
                .owner
                .generation
                .cdp()
                .command(
                    "Browser.setDownloadBehavior",
                    params,
                    Some(&self.browser_attachment),
                )
                .await?;
            if denied.get("error").is_some() {
                return Err(unavailable());
            }
        }
        if method == "Target.disposeBrowserContext"
            && reply.get("error").is_none()
            && let Some(context) = original.get("browserContextId").and_then(Value::as_str)
        {
            state.owned_contexts.remove(context);
        }
        Ok(reply)
    }

    async fn targets(&self, state: &mut State) -> Result<Vec<Value>, PlatformError> {
        let reply = self
            .owner
            .generation
            .cdp()
            .command(
                "Target.getTargets",
                json!({"filter":[{}]}),
                Some(&self.browser_attachment),
            )
            .await?;
        let reply = state.scope.reply("Target.getTargets", &json!({}), reply)?;
        reply
            .pointer("/result/targetInfos")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(unavailable)
    }

    async fn auto_attach(&self, state: &mut State, info: &Value) -> Result<(), PlatformError> {
        let Some(policy) = &state.auto_attach else {
            return Ok(());
        };
        // Native related-target ownership preserves debugger waits without touching foreign tabs.
        if info.get("type") != Some(&json!("tab")) {
            return Ok(());
        }
        let target = info
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(unavailable)?;
        if state.related_targets.contains(target) {
            return Ok(());
        }
        let mut params = json!({"targetId":state.scope.target(target)?,"waitForDebuggerOnStart":policy["waitForDebuggerOnStart"]});
        if let Some(filter) = policy.get("filter") {
            params["filter"] = filter.clone();
        }
        let reply = self
            .owner
            .generation
            .cdp()
            .command(
                "Target.autoAttachRelated",
                params,
                Some(&self.browser_attachment),
            )
            .await?;
        if reply.get("error").is_some() {
            return Err(unavailable());
        }
        state.related_targets.insert(target.to_owned());
        Ok(())
    }

    async fn event(&self, mut event: Value) -> Result<Vec<Value>, PlatformError> {
        let mut state = self.state.lock().await;
        if !self.owner.is_alive() {
            return Err(unavailable());
        }
        let Some(attachment) = event.get("sessionId").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        if attachment == self.browser_attachment {
            event
                .as_object_mut()
                .ok_or_else(unavailable)?
                .remove("sessionId");
        }
        let translated = state.scope.event(&event);
        if state.scope.download_overflow() {
            self.owner.generation.cdp().invalidate();
            return Err(unavailable());
        }
        let Some(mut event) = translated else {
            return Ok(Vec::new());
        };
        if let Some(attachment) = &state.target_attachment {
            if event.get("sessionId").and_then(Value::as_str) != Some(attachment) {
                return Ok(Vec::new());
            }
            event
                .as_object_mut()
                .ok_or_else(unavailable)?
                .remove("sessionId");
            return Ok(vec![event]);
        }
        let global = event.get("sessionId").is_none();
        if global && event["method"] == "Target.targetCreated" {
            self.auto_attach(&mut state, &event["params"]["targetInfo"])
                .await?;
        }
        if global
            && matches!(
                event["method"].as_str(),
                Some(
                    "Target.targetCreated"
                        | "Target.targetInfoChanged"
                        | "Target.targetDestroyed"
                        | "Target.targetCrashed"
                )
            )
        {
            if !state.discover {
                return Ok(Vec::new());
            }
            if let Some(info) = event.pointer("/params/targetInfo")
                && !matches_filter(&state.discovery_filter, info)
            {
                return Ok(Vec::new());
            }
        }
        Ok(vec![event])
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        let owner = self.owner.clone();
        let state = self.state.clone();
        let browser_attachment = self.browser_attachment.clone();
        let capacity = self.capacity.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _capacity = capacity;
                let _context_guard = owner.scope.lock().await;
                let mut state = state.lock().await;
                let reply = owner
                    .generation
                    .cdp()
                    .command(
                        "Target.detachFromTarget",
                        json!({"sessionId":browser_attachment}),
                        None,
                    )
                    .await;
                if matches!(reply, Ok(reply) if reply.get("error").is_none()) {
                    for context in std::mem::take(&mut state.owned_contexts) {
                        let _ = state.scope.remove_context(&context);
                    }
                } else {
                    owner.generation.cdp().invalidate();
                }
            });
        }
    }
}

fn matches_filter(filters: &[Value], info: &Value) -> bool {
    filters
        .iter()
        .find(|filter| {
            filter
                .get("type")
                .is_none_or(|kind| Some(kind) == info.get("type"))
        })
        .is_some_and(|filter| filter.get("exclude") != Some(&Value::Bool(true)))
}

fn validate_policy(params: &Value, auto_attach: bool) -> Result<(), PlatformError> {
    let fields = params.as_object().ok_or_else(unsupported)?;
    let allowed: &[&str] = if auto_attach {
        &["autoAttach", "waitForDebuggerOnStart", "flatten", "filter"]
    } else {
        &["discover", "filter"]
    };
    if fields.keys().any(|key| !allowed.contains(&key.as_str()))
        || (auto_attach
            && (params.get("flatten") != Some(&Value::Bool(true))
                || !params
                    .get("waitForDebuggerOnStart")
                    .is_some_and(Value::is_boolean)))
    {
        return Err(unsupported());
    }
    if let Some(filters) = params.get("filter") {
        let filters = filters
            .as_array()
            .filter(|filters| filters.len() <= 32)
            .ok_or_else(unsupported)?;
        for filter in filters {
            let fields = filter.as_object().ok_or_else(unsupported)?;
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "exclude"))
                || fields.get("type").is_some_and(|kind| !kind.is_string())
                || fields
                    .get("exclude")
                    .is_some_and(|exclude| !exclude.is_boolean())
            {
                return Err(unsupported());
            }
        }
    }
    Ok(())
}
fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeUnavailable, "browser client unavailable")
}
fn unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserUnsupported,
        "browser CDP method or field is unsupported",
    )
}
