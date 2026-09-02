//! Embedded operator dashboard static assets compiled into `ocd`.

mod payload {
    include!(concat!(env!("OUT_DIR"), "/embedded_dashboard.rs"));
}

/// Deterministic SHA-256 over the embedded dashboard asset tree.
#[must_use]
pub const fn embedded_dashboard_assets_sha256() -> &'static str {
    payload::ASSETS_SHA256
}

/// Logical relative paths and exact bytes for one release-owned dashboard artifact.
#[must_use]
pub fn embedded_dashboard_files() -> &'static [(&'static str, &'static [u8])] {
    payload::FILES
}
