//! Managed session ownership and scoped commands beneath a shared browser generation.

use super::{BrowserGeneration, BrowserScope};
use open_compute_core::{ErrorCode, PlatformError};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

/// One isolated default context and its explicit additional contexts/targets/attachments.
#[derive(Debug)]
pub struct ManagedBrowserSession {
    pub(super) generation: Arc<BrowserGeneration>,
    pub(super) scope: Arc<Mutex<BrowserScope>>,
    pub(super) downloads: Arc<super::downloads::BrowserDownloads>,
    download_monitor: tokio::task::AbortHandle,
    closed: Arc<AtomicBool>,
}

impl ManagedBrowserSession {
    /// Read generation-owned frontend resources only while this session remains live.
    pub fn frontend(&self) -> Result<&super::BrowserFrontend, PlatformError> {
        if !self.is_alive() {
            return Err(unavailable());
        }
        Ok(&self.generation.frontend)
    }
    /// Allocate an isolated default context without publishing its engine locator.
    pub async fn open(generation: Arc<BrowserGeneration>) -> Result<Arc<Self>, PlatformError> {
        // Completion owns any created context even when the caller abandons its acquire.
        tokio::spawn(async move {
            let reply = generation
                .cdp()
                .command("Target.createBrowserContext", json!({}), None)
                .await
                .inspect_err(|_| generation.cdp().invalidate())?;
            let context = reply
                .pointer("/result/browserContextId")
                .and_then(Value::as_str)
                .ok_or_else(unavailable)
                .inspect_err(|_| generation.cdp().invalidate())?;
            let downloads = Arc::new(
                super::downloads::BrowserDownloads::new(
                    &generation.workspace,
                    generation.max_download_bytes,
                    generation.max_download_files,
                )
                .inspect_err(|_| generation.cdp().invalidate())?,
            );
            let closed = Arc::new(AtomicBool::new(false));
            let monitor_generation = generation.clone();
            let monitor_downloads = downloads.clone();
            let monitor_closed = closed.clone();
            let monitor = tokio::spawn(async move {
                // ponytail: a 100 ms disk check can overshoot; an OS volume quota provides a hard byte ceiling.
                let mut timer = tokio::time::interval(std::time::Duration::from_millis(100));
                loop {
                    timer.tick().await;
                    if !monitor_generation.cdp().is_alive()
                        || monitor_closed.load(Ordering::Acquire)
                    {
                        break;
                    }
                    if monitor_downloads.check().is_err() && !monitor_closed.load(Ordering::Acquire)
                    {
                        monitor_generation.cdp().invalidate();
                        break;
                    }
                }
            });
            let owner = Arc::new(Self {
                scope: Arc::new(Mutex::new(BrowserScope::new(
                    context.to_owned(),
                    generation.max_download_files as usize,
                ))),
                generation,
                downloads,
                download_monitor: monitor.abort_handle(),
                closed,
            });
            let denied = owner
                .generation
                .cdp()
                .command(
                    "Browser.setDownloadBehavior",
                    json!({"behavior":"deny","browserContextId":context}),
                    None,
                )
                .await?;
            if denied.get("error").is_some() {
                return Err(unavailable());
            }
            Ok(owner)
        })
        .await
        .map_err(|_| unavailable())?
    }

    /// Destroy this session's contexts; never close the shared browser process.
    pub async fn close(self: &Arc<Self>) -> Result<(), PlatformError> {
        let owner = self.clone();
        tokio::spawn(async move { owner.close_inner().await })
            .await
            .map_err(|_| unavailable())?
    }

    async fn close_inner(&self) -> Result<(), PlatformError> {
        let mut scope = self.scope.lock().await;
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.download_monitor.abort();
        dispose(&self.generation, &mut scope)
            .await
            .and_then(|()| self.downloads.remove())
            .inspect_err(|_| self.generation.cdp().invalidate())
    }

    /// Whether session admission and the physical generation are still valid.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.generation.cdp().is_alive()
    }
}

impl Drop for ManagedBrowserSession {
    fn drop(&mut self) {
        self.download_monitor.abort();
        if !self.closed.swap(true, Ordering::AcqRel)
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let generation = self.generation.clone();
            let scope = self.scope.clone();
            let downloads = self.downloads.clone();
            runtime.spawn(async move {
                let mut scope = scope.lock().await;
                let _ = dispose(&generation, &mut scope)
                    .await
                    .and_then(|()| downloads.remove())
                    .inspect_err(|_| generation.cdp().invalidate());
            });
        }
    }
}

async fn dispose(
    generation: &BrowserGeneration,
    scope: &mut BrowserScope,
) -> Result<(), PlatformError> {
    let contexts = scope.engine_contexts()?;
    for context in contexts {
        let reply = generation
            .cdp()
            .command(
                "Target.disposeBrowserContext",
                json!({"browserContextId":context}),
                None,
            )
            .await?;
        if reply.get("error").is_some() {
            return Err(unavailable());
        }
    }
    Ok(())
}

fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeUnavailable, "browser session unavailable")
}
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
