use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::test]
async fn headless_shell_private_pipe_uses_verified_resources_and_reaps_process_group() {
    let executable = PathBuf::from(std::env::var_os("OPEN_COMPUTE_TEST_BROWSER").expect(
        "OPEN_COMPUTE_TEST_BROWSER must select the explicit chrome-headless-shell fixture",
    ));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".temp/p22-browser-process");
    std::fs::create_dir_all(&root).unwrap();
    let workspace = tempfile::Builder::new()
        .prefix("generation-")
        .tempdir_in(&root)
        .unwrap();
    let workspace = workspace.keep();
    let installation = BrowserInstallation::open(&executable, &workspace)
        .await
        .unwrap();
    assert_eq!(installation.binary_sha256.len(), 64);
    assert_eq!(installation.contract_sha256.len(), 64);
    let process = installation
        .spawn_private(&workspace, 1024 * 1024, 32, Duration::from_secs(10))
        .await
        .unwrap();
    let targets = process
        .cdp
        .command("Target.getTargets", json!({}), None)
        .await
        .unwrap();
    assert!(
        targets["result"]["targetInfos"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let created = process
        .cdp
        .command("Target.createBrowserContext", json!({}), None)
        .await
        .unwrap();
    let context = created["result"]["browserContextId"].as_str().unwrap();
    let page = process
        .cdp
        .command(
            "Target.createTarget",
            json!({"url":"about:blank","browserContextId":context}),
            None,
        )
        .await
        .unwrap();
    let target = page["result"]["targetId"].as_str().unwrap();
    let attached = process
        .cdp
        .command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let session = attached["result"]["sessionId"].as_str().unwrap();
    let evaluated = process
        .cdp
        .command(
            "Runtime.evaluate",
            json!({"expression":"1+1","returnByValue":true}),
            Some(session),
        )
        .await
        .unwrap();
    assert_eq!(evaluated["result"]["result"]["value"], 2);
    let cdp = process.cdp.clone();
    process.shutdown(Duration::from_secs(2)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while cdp.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!workspace.join("browser.lease").exists());
}
