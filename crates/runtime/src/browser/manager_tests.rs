use super::*;
use serde_json::json;

pub(in crate::browser) fn config() -> BrowserConfig {
    BrowserConfig {
        public_origin: None,
        max_sessions: 4,
        max_pending_acquires: 8,
        acquire_timeout_ms: 15_000,
        command_timeout_ms: 5_000,
        max_connections: 8,
        max_frontend_requests: 4,
        max_actions: 2,
        max_body_bytes: 1024,
        max_download_bytes: 1024 * 1024,
        max_download_files: 16,
        max_result_bytes: 1024 * 1024,
        max_message_bytes: 1024 * 1024,
        max_queued_messages: 32,
        max_history_entries: 1000,
        history_retention_ms: 86_400_000,
        backend: BrowserBackendConfig::Managed {
            executable: std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
                .expect("explicit chrome-headless-shell fixture required")
                .into(),
            browser_idle_timeout_ms: 10,
            shutdown_grace_ms: 100,
        },
    }
}

pub(in crate::browser) fn root() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".temp/p22-browser-manager");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[tokio::test]
async fn browser_manager_single_start_retention_idle_crash_and_stop_are_generation_fenced() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("instance-a")).unwrap();
    assert!(!manager.root.exists());
    let (first, second) = tokio::join!(manager.acquire(), manager.acquire());
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.contract(), second.contract());
    assert_eq!(std::fs::read_dir(&manager.root).unwrap().count(), 1);
    let old_id = first.id().to_owned();
    let old_cdp = first.cdp().clone();
    manager.reconcile().await.unwrap();
    assert!(old_cdp.is_alive());
    drop(first);
    manager.reconcile().await.unwrap();
    assert!(old_cdp.is_alive(), "another user retains the generation");
    drop(second);
    tokio::time::timeout(Duration::from_secs(5), async {
        while old_cdp.is_alive() {
            manager.reconcile().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!manager.root.join(&old_id).exists());

    let replacement = manager.acquire().await.unwrap();
    assert_ne!(replacement.id(), old_id);
    replacement
        .cdp()
        .command("Browser.close", json!({}), None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while replacement.cdp().is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.reconcile().await.unwrap();
    let earliest_start = manager.state.lock().await.restart_not_before.unwrap();
    let recovered = manager.acquire().await.unwrap();
    assert_ne!(recovered.id(), replacement.id());
    assert!(
        Instant::now() >= earliest_start,
        "a crashed generation must pass the restart backoff"
    );
    assert!(!manager.root.join(replacement.id()).exists());

    let other = BrowserManager::new(config(), root.join("instance-b")).unwrap();
    let independent = other.acquire().await.unwrap();
    assert_ne!(independent.id(), recovered.id());
    manager.shutdown().await.unwrap();
    assert!(!recovered.cdp().is_alive());
    assert!(independent.cdp().is_alive(), "instance stop is isolated");
    assert!(manager.acquire().await.is_err());
    manager.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}

#[tokio::test]
async fn browser_manager_rejects_invalid_inputs_retains_failure_and_survives_caller_cancellation() {
    let root = root().keep();
    let mut native = config();
    native.backend = BrowserBackendConfig::Cdp {
        url: "ws://127.0.0.1/devtools/browser/one".into(),
        authorization: None,
    };
    assert!(BrowserManager::new(native, root.join("native")).is_err());
    assert!(BrowserManager::new(config(), "relative".into()).is_err());
    let mut invalid = config();
    if let BrowserBackendConfig::Managed { executable, .. } = &mut invalid.backend {
        *executable = root.join("missing");
    }
    let failed = BrowserManager::new(invalid, root.join("failed")).unwrap();
    assert!(failed.acquire().await.is_err());
    assert!(
        failed.state.lock().await.restart_not_before.is_some(),
        "startup failure also establishes a retry gap"
    );
    assert_eq!(std::fs::read_dir(&failed.root).unwrap().count(), 1);
    failed.reconcile().await.unwrap();
    failed.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(&failed.root).unwrap().count(), 1);

    let mut bounded = config();
    bounded.max_pending_acquires = 1;
    let manager = BrowserManager::new(bounded, root.join("cancelled")).unwrap();
    let guard = manager.state.lock().await;
    let caller_manager = manager.clone();
    let caller = tokio::spawn(async move { caller_manager.acquire().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while manager.pending.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert_eq!(
        manager.acquire().await.unwrap_err().code(),
        ErrorCode::BrowserLimitExceeded
    );
    drop(guard);
    tokio::time::timeout(Duration::from_secs(20), async {
        while manager.pending.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let generation = manager.acquire().await.unwrap();
    assert_eq!(std::fs::read_dir(&manager.root).unwrap().count(), 1);
    let cdp = generation.cdp().clone();
    let another = manager.acquire().await.unwrap();
    assert!(Arc::ptr_eq(&generation, &another));
    manager.shutdown().await.unwrap();
    assert!(!cdp.is_alive());

    let refused = BrowserManager::new(config(), root.join("refused")).unwrap();
    crate::fsutil::create_dir_secure(&refused.root).unwrap();
    std::os::unix::fs::symlink(&root, refused.root.join(uuid::Uuid::now_v7().to_string())).unwrap();
    assert!(refused.acquire().await.is_err());
    assert!(root.is_dir());
}

#[tokio::test]
async fn browser_manager_startup_recovers_native_orphans_without_prewarming_and_rejects_bad_identity()
 {
    use command_fds::{CommandFdExt, FdMapping};
    use std::os::unix::process::CommandExt;
    use tokio::net::UnixStream;
    for via_manager in [true, false] {
        let root = root().keep();
        let instance = root.join("orphan");
        crate::fsutil::create_dir_secure(&instance).unwrap();
        let workspace = instance.join(uuid::Uuid::now_v7().to_string());
        crate::fsutil::create_dir_secure(&workspace).unwrap();
        crate::fsutil::create_dir_secure(&workspace.join("tmp")).unwrap();
        let native = config();
        let BrowserBackendConfig::Managed { executable, .. } = &native.backend else {
            unreachable!();
        };
        let installation = BrowserInstallation::open(executable, &workspace)
            .await
            .unwrap();
        let (parent_input, child_input) = std::os::unix::net::UnixStream::pair().unwrap();
        let (child_output, parent_output) = std::os::unix::net::UnixStream::pair().unwrap();
        parent_input.set_nonblocking(true).unwrap();
        parent_output.set_nonblocking(true).unwrap();
        let cdp = BrowserCdp::pipe(
            UnixStream::from_std(parent_input).unwrap(),
            UnixStream::from_std(parent_output).unwrap(),
            1024 * 1024,
            32,
            Duration::from_secs(5),
        )
        .unwrap();
        let mut command = tokio::process::Command::new(executable);
        command
            .kill_on_drop(true)
            .env_clear()
            .env("HOME", &workspace)
            .env("TMPDIR", workspace.join("tmp"))
            .current_dir(&workspace)
            .args(["--remote-debugging-pipe", "--disable-gpu", "--no-first-run"])
            .arg(format!(
                "--user-data-dir={}",
                workspace.join("profile").display()
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(
                std::fs::File::create(workspace.join("orphan.stderr.log")).unwrap(),
            ));
        command
            .as_std_mut()
            .process_group(0)
            .fd_mappings(vec![
                FdMapping {
                    parent_fd: child_input.into(),
                    child_fd: 3,
                },
                FdMapping {
                    parent_fd: child_output.into(),
                    child_fd: 4,
                },
            ])
            .unwrap();
        let mut child = command.spawn().unwrap();
        drop(command); // The fixture must release its copies of the child pipe ends.
        let pid = i32::try_from(child.id().unwrap()).unwrap();
        assert!(
            cdp.command("Browser.getVersion", json!({}), None)
                .await
                .unwrap()
                .get("error")
                .is_none()
        );
        let path = workspace.join("browser.lease");
        let lease =
            crate::lease::capture_lease(pid, pid, &installation.binary_sha256, &path).unwrap();
        let mut corrupt = lease.clone();
        corrupt.start_key.push_str("-wrong-start");
        crate::lease::write_lease(&path, &corrupt).unwrap();
        let rejected = if via_manager {
            BrowserManager::new(native.clone(), instance.clone()).map(|_| ())
        } else {
            BrowserManager::recover_orphans(&instance).map(|_| ())
        };
        assert!(rejected.is_err());
        assert!(
            path.exists(),
            "a mismatched live lease must remain as evidence"
        );
        assert!(
            cdp.command("Browser.getVersion", json!({}), None)
                .await
                .unwrap()
                .get("error")
                .is_none(),
            "an unverifiable process must not be signaled"
        );
        std::fs::copy(&path, workspace.join("rejected-start-identity.json")).unwrap();
        crate::lease::write_lease(&path, &lease).unwrap();
        let recovered = if via_manager {
            Some(BrowserManager::new(native, instance).unwrap())
        } else {
            assert!(BrowserManager::recover_orphans(&instance).unwrap());
            None
        };
        if let Some(manager) = &recovered {
            assert!(manager.state.lock().await.recovered);
            assert!(
                manager.state.lock().await.running.is_none(),
                "orphan recovery must not prewarm a browser"
            );
        }
        assert!(!path.exists());
        assert!(
            workspace.is_dir(),
            "retained orphan profiles are not disposable successful-run data"
        );
        let _ = child.wait().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while cdp.is_alive() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if let Some(manager) = recovered {
            manager.shutdown().await.unwrap();
        }
    }
}
