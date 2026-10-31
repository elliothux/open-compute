use crate::embedded_dashboard::content_type;

#[test]
fn embedded_dashboard_content_types() {
    for (path, expected) in [
        ("index.css", "text/css; charset=utf-8"),
        ("index.htm", "text/html; charset=utf-8"),
        ("index.HTML", "text/html; charset=utf-8"),
        ("index.js", "text/javascript; charset=utf-8"),
        ("index.mjs", "text/javascript; charset=utf-8"),
        ("data.json", "application/json; charset=utf-8"),
        ("index.js.map", "application/json; charset=utf-8"),
        ("logo.svg", "image/svg+xml"),
        ("logo.png", "image/png"),
        ("photo.jpg", "image/jpeg"),
        ("photo.jpeg", "image/jpeg"),
        ("image.gif", "image/gif"),
        ("image.webp", "image/webp"),
        ("image.avif", "image/avif"),
        ("favicon.ico", "image/x-icon"),
        ("font.woff", "font/woff"),
        ("font.woff2", "font/woff2"),
        ("robots.txt", "text/plain; charset=utf-8"),
        ("module.wasm", "application/wasm"),
        ("LICENSE", "application/octet-stream"),
        ("archive.bin", "application/octet-stream"),
    ] {
        assert_eq!(content_type(path), expected, "path={path}");
    }
}
