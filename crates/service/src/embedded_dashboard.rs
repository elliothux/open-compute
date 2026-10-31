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

pub(crate) fn content_type(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let extension = filename
        .rsplit('.')
        .next()
        .filter(|_| filename.contains('.'))
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "css" => "text/css; charset=utf-8",
        "htm" | "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
    .to_owned()
}
