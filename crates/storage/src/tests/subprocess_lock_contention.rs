use super::*;

#[test]
fn subprocess_lock_contention() {
    if std::env::var("PLATFORM_STORAGE_HOLD_LOCK").ok().as_deref() == Some("1") {
        let root = PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_ROOT").unwrap());
        let config = storage_config(&root);
        let _owned = DataDir::acquire(&config).expect("child lock");
        let ready = PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_READY").unwrap());
        File::create(&ready).unwrap();
        loop {
            if PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_STOP").unwrap()).exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        return;
    }

    let (_tmp, root) = unique_root();
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let ready = _tmp.path().join("ready");
    let stop = _tmp.path().join("stop");
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe)
        .env("PLATFORM_STORAGE_HOLD_LOCK", "1")
        .env("PLATFORM_STORAGE_HOLD_ROOT", &root)
        .env("PLATFORM_STORAGE_HOLD_READY", &ready)
        .env("PLATFORM_STORAGE_HOLD_STOP", &stop)
        .args([
            "--exact",
            "tests::subprocess_lock_contention::subprocess_lock_contention",
            "--nocapture",
        ])
        .spawn()
        .expect("spawn");
    let start = std::time::Instant::now();
    while !ready.exists() {
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("child did not acquire lock");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("contended");
    assert_eq!(err.code(), ErrorCode::DataDirInUse);
    File::create(&stop).unwrap();
    let _ = child.wait();
    DataDir::acquire(&config).expect("after child release");
}
