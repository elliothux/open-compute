use super::*;
use crate::browser::manager::tests::{config, root};
use crate::browser::{BrowserManager, ManagedBrowserSession};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn downloads_fail_closed_on_directory_flood_permissions_and_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;
    let root = root().keep();
    let owner = BrowserDownloads::new(&root, 4, 2).unwrap();
    for index in 0..4_097 {
        std::fs::create_dir(owner.root.join(index.to_string())).unwrap();
    }
    assert!(owner.check().is_err());
    owner.remove().unwrap();
    let owner = BrowserDownloads::new(&root, 4, 2).unwrap();
    let mut params =
        json!({"browserContextId":"owned","behavior":"allowAndName","downloadPath":"/tmp/client"});
    owner.configure(&mut params).unwrap();
    let context = PathBuf::from(params["downloadPath"].as_str().unwrap());
    std::fs::write(context.join("file"), b"1").unwrap();
    std::fs::set_permissions(&owner.root, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(owner.check().is_err());
    assert!(owner.remove_context("owned").is_err());
    std::fs::set_permissions(&owner.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&context, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(owner.check().is_err());
    std::fs::set_permissions(&context, std::fs::Permissions::from_mode(0o700)).unwrap();
    owner.remove().unwrap();
    let workspace = root.join(std::ffi::OsString::from_vec(vec![0xff]));
    // APFS rejects this name; Unix filesystems may accept it, but CDP paths must be UTF-8.
    if crate::fsutil::create_dir_secure(&workspace).is_ok() {
        let owner = BrowserDownloads::new(&workspace, 4, 2).unwrap();
        assert!(owner.configure(&mut params).is_err());
        owner.remove().unwrap();
    } else {
        assert!(BrowserDownloads::new(&workspace, 4, 2).is_err());
    }
    assert!(BrowserDownloads::new(&root.join("absent"), 4, 2).is_err());
}

#[test]
fn downloads_own_paths_byte_file_and_symlink_boundaries() {
    let root = root().keep();
    let owner = BrowserDownloads::new(&root, 4, 2).unwrap();
    let mut params = json!({"browserContextId":"native-context","behavior":"allowAndName","downloadPath":"/private/platform-authority"});
    owner.configure(&mut params).unwrap();
    let path = PathBuf::from(params["downloadPath"].as_str().unwrap());
    assert!(path.starts_with(&root));
    assert!(!path.to_string_lossy().contains("native-context"));
    std::fs::write(path.join("first"), b"1234").unwrap();
    owner.check().unwrap();
    std::fs::write(path.join("second"), b"5").unwrap();
    assert!(owner.check().is_err());
    std::fs::write(path.join("first"), b"").unwrap();
    owner.check().unwrap();
    std::fs::write(path.join("third"), b"").unwrap();
    assert!(owner.check().is_err());
    owner.remove_context("native-context").unwrap();
    owner.remove_context("native-context").unwrap();
    owner.check().unwrap();
    owner.configure(&mut params).unwrap();
    let canary = root.join("canary");
    std::fs::write(&canary, b"secret").unwrap();
    std::os::unix::fs::symlink(&canary, path.join("escape")).unwrap();
    assert!(owner.check().is_err());
    owner.remove_context("native-context").unwrap();
    assert_eq!(std::fs::read(canary).unwrap(), b"secret");
    std::os::unix::fs::symlink(root.join("absent"), owner.root.join("broken")).unwrap();
    assert!(owner.check().is_err());
    owner.remove().unwrap();
    let denied = BrowserDownloads::new(&root, 4, 2).unwrap();
    let mut params = json!({"browserContextId":"native-context","behavior":"deny","downloadPath":"/tmp/client-artifact"});
    denied.configure(&mut params).unwrap();
    assert!(params.get("downloadPath").is_none());
    assert!(denied.configure(&mut json!({"behavior":"deny"})).is_err());
    denied.remove().unwrap();
}

#[tokio::test]
async fn managed_downloads_are_native_owned_scoped_and_cleaned_before_release() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("generation")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let first = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let second = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let client = first.connect(None).await.unwrap();
    let foreign = second.connect(None).await.unwrap();
    let mut events = client.subscribe();
    let context = client
        .command("Target.createBrowserContext", json!({}), None)
        .await
        .unwrap()["result"]["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = client.command("Browser.setDownloadBehavior", json!({"behavior":"allowAndName","eventsEnabled":true,"browserContextId":context,"downloadPath":"/private/control-authority"}), None).await.unwrap();
    assert!(response.get("error").is_none(), "{response}");
    let target = client
        .command(
            "Target.createTarget",
            json!({"url":"about:blank","browserContextId":context}),
            None,
        )
        .await
        .unwrap()["result"]["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let attachment = client
        .command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .await
        .unwrap()["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    client
        .command("Page.enable", json!({}), Some(&attachment))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let mut used = 0;
        loop {
            let amount = socket.read(&mut request[used..]).await.unwrap();
            assert!(amount > 0);
            used += amount;
            if request[..used].windows(4).any(|value| value == b"\r\n\r\n") {
                break;
            }
            assert!(used < request.len());
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=fixture.txt\r\nContent-Length: 12\r\nConnection: close\r\n\r\nnative bytes").await.unwrap();
    });
    client
        .command(
            "Page.navigate",
            json!({"url":format!("http://{address}/download")}),
            Some(&attachment),
        )
        .await
        .unwrap();
    let mut guid = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["method"] == "Browser.downloadWillBegin" {
                guid = event["params"]["guid"].as_str().unwrap().to_owned();
                assert_eq!(event["params"]["frameId"], target);
            }
            if event["method"] == "Browser.downloadProgress"
                && event["params"]["state"] == "completed"
            {
                assert_eq!(event["params"]["guid"], guid);
                assert!(event["params"].get("filePath").is_none());
                break;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();
    let directory = std::fs::read_dir(&first.downloads.root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let file = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(file).unwrap(), b"native bytes");
    assert!(
        foreign
            .command("Browser.cancelDownload", json!({"guid":guid}), None)
            .await
            .unwrap()
            .get("error")
            .is_some()
    );
    assert!(foreign.command("Browser.setDownloadBehavior", json!({"behavior":"allowAndName","browserContextId":context,"downloadPath":"/tmp/client"}), None).await.unwrap().get("error").is_some());
    client
        .command(
            "Target.disposeBrowserContext",
            json!({"browserContextId":context}),
            None,
        )
        .await
        .unwrap();
    assert!(!directory.exists());
    let first_root = first.downloads.root.clone();
    first.close().await.unwrap();
    assert!(!first_root.exists());
    assert!(second.is_alive());
    second.close().await.unwrap();
    drop((client, foreign, first, second, generation));
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn download_quota_fences_the_generation_and_drop_cleans_the_session_directory() {
    let root = root().keep();
    let mut limits = config();
    limits.max_download_bytes = 1;
    let manager = BrowserManager::new(limits, root.join("generation")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let session = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let directory = session.downloads.root.clone();
    let mut params = json!({"behavior":"allowAndName","browserContextId":"owned-test-directory"});
    session.downloads.configure(&mut params).unwrap();
    std::fs::write(
        PathBuf::from(params["downloadPath"].as_str().unwrap()).join("download"),
        b"too large",
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while session.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.reconcile().await.unwrap();
    assert!(!directory.exists());
    drop((session, generation));
    manager.shutdown().await.unwrap();

    let manager = BrowserManager::new(config(), root.join("drop-generation")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let session = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let directory = session.downloads.root.clone();
    let weak = Arc::downgrade(&session);
    drop(session);
    tokio::time::timeout(Duration::from_secs(3), async {
        while directory.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(weak.upgrade().is_none());
    assert!(generation.cdp().is_alive());
    drop(generation);
    manager.shutdown().await.unwrap();
}
