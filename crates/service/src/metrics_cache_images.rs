//! Fixed-series Workers Cache and Images metrics with no tenant or content labels.

use super::{Inner, MetricsRegistry, write_help};
use open_compute_storage::CacheStats;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct CacheImagesMetrics {
    cache_operations: [[u64; 2]; 5],
    cache_stats: CacheStats,
    cache_s3_buckets: [[u64; 5]; 2],
    cache_s3_sum: [f64; 2],
    cache_s3_count: [u64; 2],
    image_operations: [[u64; 3]; 5],
    image_active_sessions: u64,
    image_active_transforms: u64,
    image_bytes: [u64; 2],
    image_limit_rejections: [u64; 5],
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CacheMetricOperation {
    Lookup,
    Store,
    Delete,
    Purge,
    Refresh,
}

impl CacheMetricOperation {
    const fn index(self) -> usize {
        match self {
            Self::Lookup => 0,
            Self::Store => 1,
            Self::Delete => 2,
            Self::Purge => 3,
            Self::Refresh => 4,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Lookup => "lookup",
            Self::Store => "store",
            Self::Delete => "delete",
            Self::Purge => "purge",
            Self::Refresh => "refresh",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CacheS3Operation {
    Get,
    Put,
}

impl CacheS3Operation {
    const fn index(self) -> usize {
        match self {
            Self::Get => 0,
            Self::Put => 1,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Put => "put",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageMetricOperation {
    Input,
    Info,
    Transform,
    Draw,
    Output,
}

impl ImageMetricOperation {
    const fn index(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Info => 1,
            Self::Transform => 2,
            Self::Draw => 3,
            Self::Output => 4,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Info => "info",
            Self::Transform => "transform",
            Self::Draw => "draw",
            Self::Output => "output",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageMetricOutcome {
    Success,
    Failure,
    Limit,
}

impl ImageMetricOutcome {
    const fn index(self) -> usize {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Limit => 2,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Limit => "limit",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ImageTransformGuard {
    metrics: Arc<MetricsRegistry>,
}

impl Drop for ImageTransformGuard {
    fn drop(&mut self) {
        let mut inner = self.metrics.lock();
        inner.cache_images.image_active_transforms =
            inner.cache_images.image_active_transforms.saturating_sub(1);
    }
}

impl MetricsRegistry {
    pub(crate) fn observe_response_cache(&self, operation: CacheMetricOperation, success: bool) {
        let mut inner = self.lock();
        let count =
            &mut inner.cache_images.cache_operations[operation.index()][usize::from(!success)];
        *count = count.saturating_add(1);
    }

    pub(crate) fn set_response_cache_stats(&self, stats: CacheStats) {
        self.lock().cache_images.cache_stats = stats;
    }

    pub(crate) fn observe_response_cache_s3(
        &self,
        operation: CacheS3Operation,
        duration: Duration,
    ) {
        let seconds = duration.as_secs_f64();
        let index = operation.index();
        let mut inner = self.lock();
        let metrics = &mut inner.cache_images;
        metrics.cache_s3_count[index] = metrics.cache_s3_count[index].saturating_add(1);
        metrics.cache_s3_sum[index] += seconds;
        for (bucket, upper) in [0.01, 0.1, 1.0, 10.0, f64::INFINITY]
            .into_iter()
            .enumerate()
        {
            if seconds <= upper {
                metrics.cache_s3_buckets[index][bucket] =
                    metrics.cache_s3_buckets[index][bucket].saturating_add(1);
            }
        }
    }

    pub(crate) fn observe_image(
        &self,
        operation: ImageMetricOperation,
        outcome: ImageMetricOutcome,
    ) {
        let mut inner = self.lock();
        let metrics = &mut inner.cache_images;
        let count = &mut metrics.image_operations[operation.index()][outcome.index()];
        *count = count.saturating_add(1);
        if matches!(outcome, ImageMetricOutcome::Limit) {
            let rejected = &mut metrics.image_limit_rejections[operation.index()];
            *rejected = rejected.saturating_add(1);
        }
    }

    pub(crate) fn add_image_bytes(&self, input: bool, bytes: u64) {
        let mut inner = self.lock();
        let value = &mut inner.cache_images.image_bytes[usize::from(!input)];
        *value = value.saturating_add(bytes);
    }

    pub(crate) fn set_image_active_sessions(&self, sessions: u64) {
        self.lock().cache_images.image_active_sessions = sessions;
    }

    pub(crate) fn image_transform(self: &Arc<Self>) -> ImageTransformGuard {
        let mut inner = self.lock();
        inner.cache_images.image_active_transforms =
            inner.cache_images.image_active_transforms.saturating_add(1);
        drop(inner);
        ImageTransformGuard {
            metrics: self.clone(),
        }
    }
}

pub(super) fn write_cache_images_metrics(out: &mut String, inner: &Inner) {
    let metrics = &inner.cache_images;
    write_help(
        out,
        "response_cache_operations_total",
        "counter",
        "Response cache operation outcomes",
    );
    for operation in cache_operations() {
        for (outcome, index) in [("success", 0), ("failure", 1)] {
            writeln!(
                out,
                "response_cache_operations_total{{operation=\"{}\",outcome=\"{outcome}\"}} {}",
                operation.as_str(),
                metrics.cache_operations[operation.index()][index]
            )
            .ok();
        }
    }
    for (name, help, value) in [
        (
            "response_cache_metadata_bytes",
            "Response cache SQLite bytes",
            metrics.cache_stats.metadata_bytes,
        ),
        (
            "response_cache_body_bytes",
            "Response cache logical body bytes",
            metrics.cache_stats.body_bytes,
        ),
        (
            "response_cache_active_refreshes",
            "Response cache live refresh leases",
            metrics.cache_stats.active_refreshes,
        ),
        (
            "response_cache_open_databases",
            "Response cache process-local database handles",
            metrics.cache_stats.open_databases,
        ),
    ] {
        write_help(out, name, "gauge", help);
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "response_cache_s3_duration_seconds",
        "histogram",
        "Response cache immutable body S3 latency",
    );
    for operation in [CacheS3Operation::Get, CacheS3Operation::Put] {
        let index = operation.index();
        for (bucket, label) in ["0.01", "0.1", "1", "10", "+Inf"].into_iter().enumerate() {
            writeln!(
                out,
                "response_cache_s3_duration_seconds_bucket{{operation=\"{}\",le=\"{label}\"}} {}",
                operation.as_str(),
                metrics.cache_s3_buckets[index][bucket]
            )
            .ok();
        }
        writeln!(
            out,
            "response_cache_s3_duration_seconds_sum{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.cache_s3_sum[index]
        )
        .ok();
        writeln!(
            out,
            "response_cache_s3_duration_seconds_count{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.cache_s3_count[index]
        )
        .ok();
    }
    write_help(
        out,
        "images_operations_total",
        "counter",
        "Images operation outcomes",
    );
    for operation in image_operations() {
        for outcome in [
            ImageMetricOutcome::Success,
            ImageMetricOutcome::Failure,
            ImageMetricOutcome::Limit,
        ] {
            writeln!(
                out,
                "images_operations_total{{operation=\"{}\",outcome=\"{}\"}} {}",
                operation.as_str(),
                outcome.as_str(),
                metrics.image_operations[operation.index()][outcome.index()]
            )
            .ok();
        }
    }
    for (name, help, value) in [
        (
            "images_active_sessions",
            "Retained Images sessions",
            metrics.image_active_sessions,
        ),
        (
            "images_active_transforms",
            "Active native image transforms",
            metrics.image_active_transforms,
        ),
    ] {
        write_help(out, name, "gauge", help);
        writeln!(out, "{name} {value}").ok();
    }
    write_help(
        out,
        "images_bytes_total",
        "counter",
        "Images input and output bytes",
    );
    for (direction, index) in [("input", 0), ("output", 1)] {
        writeln!(
            out,
            "images_bytes_total{{direction=\"{direction}\"}} {}",
            metrics.image_bytes[index]
        )
        .ok();
    }
    write_help(
        out,
        "images_limit_rejections_total",
        "counter",
        "Images limit rejections by operation",
    );
    for operation in image_operations() {
        writeln!(
            out,
            "images_limit_rejections_total{{operation=\"{}\"}} {}",
            operation.as_str(),
            metrics.image_limit_rejections[operation.index()]
        )
        .ok();
    }
}

const fn cache_operations() -> [CacheMetricOperation; 5] {
    [
        CacheMetricOperation::Lookup,
        CacheMetricOperation::Store,
        CacheMetricOperation::Delete,
        CacheMetricOperation::Purge,
        CacheMetricOperation::Refresh,
    ]
}

const fn image_operations() -> [ImageMetricOperation; 5] {
    [
        ImageMetricOperation::Input,
        ImageMetricOperation::Info,
        ImageMetricOperation::Transform,
        ImageMetricOperation::Draw,
        ImageMetricOperation::Output,
    ]
}

#[cfg(test)]
#[path = "metrics_cache_images_tests.rs"]
mod tests;
