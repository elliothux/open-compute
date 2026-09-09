use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_digest_cache_lookup_cannot_delete_publish_window() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let platform = platform_meta();
    let dest_name = {
        let (digest, _, _) = digest_for(
            dir.path(),
            runtime.lock_bytes(),
            &runtime,
            &platform,
            &token,
            &TEST_BINDING_TOKEN,
            &TEST_OBSERVABILITY_TOKEN,
        )
        .unwrap();
        format!("config.{digest}.bin")
    };
    let dest = data.join(&dest_name);
    let sidecar = dest.with_extension("bin.digest");
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    set_after_config_rename_hook({
        let dest = dest.clone();
        let paused = paused.clone();
        let release = release.clone();
        move |path| {
            if path == dest {
                paused.store(true, std::sync::atomic::Ordering::SeqCst);
                let wait_from = std::time::Instant::now();
                while !release.load(std::sync::atomic::Ordering::SeqCst)
                    && wait_from.elapsed() < Duration::from_secs(5)
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    });
    let a = tokio::spawn({
        let runtime = runtime.clone();
        let lock_path = lock_path.clone();
        let assets = dir.path().to_path_buf();
        let data = data.clone();
        let platform = platform.clone();
        let token = SecretString::new(TOKEN);
        async move {
            let redactor = redactor_with_token();
            compile_static_config(compile_req(
                &runtime,
                &lock_path,
                &assets,
                &data,
                &platform,
                &token,
                &redactor,
                Duration::from_secs(8),
            ))
            .await
        }
    });
    let wait_from = std::time::Instant::now();
    while !paused.load(std::sync::atomic::Ordering::SeqCst)
        && wait_from.elapsed() < Duration::from_secs(5)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        paused.load(std::sync::atomic::Ordering::SeqCst),
        "publisher A must pause after config rename"
    );
    assert!(dest.exists(), "publisher A must have renamed the config");
    assert!(
        !sidecar.exists(),
        "sidecar must not exist while A is paused"
    );
    let c = tokio::spawn({
        let runtime = runtime.clone();
        let lock_path = lock_path.clone();
        let assets = dir.path().to_path_buf();
        let data = data.clone();
        let platform = platform.clone();
        let token = SecretString::new(TOKEN);
        let c_started = c_started.clone();
        async move {
            c_started.store(true, std::sync::atomic::Ordering::SeqCst);
            let redactor = redactor_with_token();
            compile_static_config(compile_req(
                &runtime,
                &lock_path,
                &assets,
                &data,
                &platform,
                &token,
                &redactor,
                Duration::from_secs(8),
            ))
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        c_started.load(std::sync::atomic::Ordering::SeqCst),
        "caller C must have started"
    );
    assert!(
        !c.is_finished(),
        "caller C must block on the digest gate instead of deleting A's transient config"
    );
    assert!(dest.exists());
    assert!(!sidecar.exists());
    assert!(!a.is_finished(), "publisher A must still hold the gate");
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    let (ra, rc) = tokio::join!(a, c);
    clear_after_config_rename_hook();
    let ca = ra.expect("join A").expect("publisher A must succeed");
    let cc = rc.expect("join C").expect("caller C must reuse the winner");
    assert_eq!(ca.digest(), cc.digest());
    ca.open().expect("winner must revalidate");
    cc.open().expect("caller must revalidate the same winner");
}
