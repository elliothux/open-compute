//! Fixed Browser Run operation metrics; tenant content never becomes a label.

use super::{Inner, MetricsRegistry, write_help};
use open_compute_core::{ErrorCode, PlatformError};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub(crate) enum BrowserOperation {
    Acquire,
    Connect,
    Command,
    Close,
    Content,
    Screenshot,
    Pdf,
    Snapshot,
    Scrape,
    Links,
    Markdown,
    Json,
    AccessibilityTree,
}

impl BrowserOperation {
    const ALL: [Self; 13] = [
        Self::Acquire,
        Self::Connect,
        Self::Command,
        Self::Close,
        Self::Content,
        Self::Screenshot,
        Self::Pdf,
        Self::Snapshot,
        Self::Scrape,
        Self::Links,
        Self::Markdown,
        Self::Json,
        Self::AccessibilityTree,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
            Self::Connect => "connect",
            Self::Command => "command",
            Self::Close => "close",
            Self::Content => "content",
            Self::Screenshot => "screenshot",
            Self::Pdf => "pdf",
            Self::Snapshot => "snapshot",
            Self::Scrape => "scrape",
            Self::Links => "links",
            Self::Markdown => "markdown",
            Self::Json => "json",
            Self::AccessibilityTree => "accessibility_tree",
        }
    }

    pub(crate) fn action(action: &str) -> Option<Self> {
        match action {
            "content" => Some(Self::Content),
            "screenshot" => Some(Self::Screenshot),
            "pdf" => Some(Self::Pdf),
            "snapshot" => Some(Self::Snapshot),
            "scrape" => Some(Self::Scrape),
            "links" => Some(Self::Links),
            "markdown" => Some(Self::Markdown),
            "json" => Some(Self::Json),
            "accessibilityTree" => Some(Self::AccessibilityTree),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BrowserOutcome {
    Success,
    Failure,
    Limit,
    Timeout,
    Cancelled,
}

impl BrowserOutcome {
    const ALL: [Self; 5] = [
        Self::Success,
        Self::Failure,
        Self::Limit,
        Self::Timeout,
        Self::Cancelled,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Limit => "limit",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn result<T>(result: &Result<T, PlatformError>) -> Self {
        match result {
            Ok(_) => Self::Success,
            Err(error) if error.code() == ErrorCode::BrowserLimitExceeded => Self::Limit,
            Err(error) if error.code() == ErrorCode::BrowserTimeout => Self::Timeout,
            Err(_) => Self::Failure,
        }
    }

    pub(crate) fn status(status: axum::http::StatusCode) -> Self {
        match status {
            axum::http::StatusCode::TOO_MANY_REQUESTS
            | axum::http::StatusCode::PAYLOAD_TOO_LARGE => Self::Limit,
            axum::http::StatusCode::GATEWAY_TIMEOUT => Self::Timeout,
            _ if status.is_success() => Self::Success,
            _ => Self::Failure,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct BrowserMetrics {
    requests: [[u64; 5]; 13],
    duration: [f64; 13],
    in_flight: [u64; 13],
    sessions: u64,
}

#[derive(Debug)]
pub(crate) struct BrowserOperationGuard {
    registry: Arc<MetricsRegistry>,
    operation: BrowserOperation,
    outcome: BrowserOutcome,
    started: Instant,
}

impl BrowserOperationGuard {
    pub(crate) fn finish(mut self, outcome: BrowserOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for BrowserOperationGuard {
    fn drop(&mut self) {
        let mut inner = self.registry.lock();
        let metrics = &mut inner.browser;
        let index = self.operation.index();
        metrics.in_flight[index] = metrics.in_flight[index].saturating_sub(1);
        metrics.duration[index] += self.started.elapsed().as_secs_f64();
        let count = &mut metrics.requests[index][self.outcome as usize];
        *count = count.saturating_add(1);
    }
}

impl MetricsRegistry {
    pub(crate) fn browser_operation(
        self: &Arc<Self>,
        operation: BrowserOperation,
    ) -> BrowserOperationGuard {
        let mut inner = self.lock();
        let count = &mut inner.browser.in_flight[operation.index()];
        *count = count.saturating_add(1);
        drop(inner);
        BrowserOperationGuard {
            registry: self.clone(),
            operation,
            outcome: BrowserOutcome::Cancelled,
            started: Instant::now(),
        }
    }

    pub(crate) fn set_browser_sessions(&self, sessions: u64) {
        self.lock().browser.sessions = sessions;
    }
}

pub(super) fn write_browser_metrics(out: &mut String, inner: &Inner) {
    let metrics = &inner.browser;
    write_help(
        out,
        "browser_operations_total",
        "counter",
        "Browser operation outcomes, including caller cancellation",
    );
    for operation in BrowserOperation::ALL {
        for outcome in BrowserOutcome::ALL {
            writeln!(
                out,
                "browser_operations_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome.as_str(),
                metrics.requests[operation.index()][outcome as usize]
            )
            .ok();
        }
    }
    write_help(
        out,
        "browser_operation_duration_seconds",
        "summary",
        "Browser caller operation elapsed time",
    );
    for operation in BrowserOperation::ALL {
        let index = operation.index();
        let count = metrics.requests[index]
            .iter()
            .fold(0_u64, |sum, count| sum.saturating_add(*count));
        writeln!(
            out,
            "browser_operation_duration_seconds_sum{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.duration[index]
        )
        .ok();
        writeln!(
            out,
            "browser_operation_duration_seconds_count{{operation=\"{}\"}} {count}",
            operation.as_str()
        )
        .ok();
    }
    write_help(
        out,
        "browser_in_flight_operations",
        "gauge",
        "Live caller operations; cancelled native cleanup may continue separately",
    );
    for operation in BrowserOperation::ALL {
        writeln!(
            out,
            "browser_in_flight_operations{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.in_flight[operation.index()]
        )
        .ok();
    }
    write_help(
        out,
        "browser_active_sessions",
        "gauge",
        "Locally owned Browser Run sessions",
    );
    writeln!(out, "browser_active_sessions {}", metrics.sessions).ok();
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
