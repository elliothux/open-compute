use super::*;
use flate2::{Compression, write::GzEncoder};
use serde_json::json;
use std::io::Write;

const BINARY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VERSION: &str = "Google Chrome for Testing 153.0.8010.12";

fn bundle() -> Value {
    let assets: Vec<_> = ["inspector.html", "entrypoints/inspector/inspector.js", "LICENSE.headless_shell"]
        .into_iter().map(|path| json!({"path":path,"sha256":hex::encode(Sha256::digest(b"native resource")),"mediaType":"text/plain; charset=utf-8","data":STANDARD.encode(b"native resource")})).collect();
    json!({"binarySha256":BINARY,"version":VERSION,"revision":format!("@{}","a".repeat(40)),"protocol":{"version":{"major":"1","minor":"3"},"domains":[{"domain":"Page"}]},"assets":assets})
}

fn write(root: &Path, value: &Value) -> std::path::PathBuf {
    let path = root.join("browser-devtools.json.gz");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[test]
fn prepared_frontend_is_immutable_bounded_and_bound_to_native_version() {
    let root = crate::browser::manager::tests::root();
    let path = write(root.path(), &bundle());
    let frontend = BrowserFrontend::open(&path, BINARY, VERSION).unwrap();
    let (bytes, media) = frontend.asset("inspector.html", 1024).unwrap();
    assert_eq!(bytes, b"native resource");
    assert_eq!(media, "text/plain; charset=utf-8");
    assert_eq!(frontend.sha256.len(), 64);
    assert_eq!(frontend.protocol()["domains"][0]["domain"], "Page");
    assert_eq!(
        frontend.asset("inspector.html", 1).unwrap_err().code(),
        ErrorCode::BrowserLimitExceeded
    );
    assert_eq!(
        frontend
            .asset("../inspector.html", 1024)
            .unwrap_err()
            .code(),
        ErrorCode::BrowserUnsupported
    );
    let version = json!({"result":{"product":"HeadlessChrome/153.0.8010.12","revision":format!("@{}","a".repeat(40))}});
    assert!(frontend.matches_version(&version));
    assert!(!frontend.matches_version(
        &json!({"result":{"product":"HeadlessChrome/153.0.8010.12","revision":"@foreign"}})
    ));
    assert!(!frontend.matches_version(&json!({"error":{}})));
    std::fs::write(&path, b"replaced").unwrap();
    assert_eq!(
        frontend.asset("inspector.html", 1024).unwrap().0,
        b"native resource"
    );
    assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
}

#[test]
fn prepared_frontend_rejects_trailing_bytes_and_decompression_overflow() {
    let root = crate::browser::manager::tests::root();
    let path = write(root.path(), &bundle());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"unexpected suffix");
    std::fs::write(&path, bytes).unwrap();
    assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&vec![b' '; MAX_DECODED as usize + 1])
        .unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
}

#[tokio::test]
async fn prepared_native_frontend_is_available_only_for_a_live_managed_session() {
    use crate::browser::{BrowserManager, ManagedBrowserSession};
    let root = crate::browser::manager::tests::root();
    let manager = BrowserManager::new(
        crate::browser::manager::tests::config(),
        root.path().join("native-frontend"),
    )
    .unwrap();
    let session = ManagedBrowserSession::open(manager.acquire().await.unwrap())
        .await
        .unwrap();
    let frontend = session.frontend().unwrap();
    let (html, media) = frontend.asset("inspector.html", MAX_ASSET).unwrap();
    assert!(std::str::from_utf8(html).unwrap().contains("inspector.js"));
    assert_eq!(media, "text/html; charset=utf-8");
    assert!(
        frontend.protocol()["domains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|domain| domain["domain"] == "Target")
    );
    session.close().await.unwrap();
    assert!(session.frontend().is_err());
    manager.shutdown().await.unwrap();
}

#[test]
fn prepared_frontend_rejects_corrupt_unsafe_or_mismatched_inputs() {
    let root = crate::browser::manager::tests::root();
    let path = write(root.path(), &bundle());
    assert!(BrowserFrontend::open(&path, &"b".repeat(64), VERSION).is_err());
    assert!(BrowserFrontend::open(&path, BINARY, "Google Chrome for Testing 154.0.0.0").is_err());
    assert!(BrowserFrontend::open(&path, BINARY, "invalid").is_err());
    let mut invalids = Vec::new();
    for (pointer, value) in [
        ("/revision", json!("not-a-revision")),
        ("/protocol/version/major", json!("2")),
        ("/protocol/version/minor", json!("4")),
        ("/protocol/domains", json!([])),
        ("/assets/0/path", json!("../outside")),
        ("/assets/0/path", json!("unsafe\\path")),
        ("/assets/0/mediaType", json!("text/html\r\nInjected: yes")),
        ("/assets/0/data", json!("invalid base64!")),
        ("/assets/0/data", json!("")),
        ("/assets/0/sha256", json!("0".repeat(64))),
    ] {
        let mut broken = bundle();
        *broken.pointer_mut(pointer).unwrap() = value;
        invalids.push(broken);
    }
    let mut missing = bundle();
    missing["assets"].as_array_mut().unwrap().pop();
    invalids.push(missing);
    let mut duplicated = bundle();
    let duplicate = duplicated["assets"][0].clone();
    duplicated["assets"].as_array_mut().unwrap().push(duplicate);
    invalids.push(duplicated);
    let mut unknown = bundle();
    unknown["unexpected"] = json!(true);
    invalids.push(unknown);
    for broken in invalids {
        let path = write(root.path(), &broken);
        assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
    }
    let path = write(root.path(), &bundle());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o622)).unwrap();
    assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.path().join("missing"), &path).unwrap();
    assert!(BrowserFrontend::open(&path, BINARY, VERSION).is_err());
}
