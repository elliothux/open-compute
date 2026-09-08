use super::*;

#[tokio::test]
async fn supervisor_construction_debug_and_default_wiring_are_secret_safe() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let binary = dir.path().join("workerd");
    write_exec(&binary, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&binary));
    let runtime = verify_ok(&lock_path, &binary).await;
    let data = dir.path().join("runtime-data");
    fs::create_dir(&data).unwrap();
    let compiler = crate::StaticConfigCompiler::new(
        runtime.clone(),
        lock_path,
        dir.path().to_path_buf(),
        data,
        platform_meta(),
        Duration::from_secs(1),
        Redactor::new(),
    );
    assert!(format!("{compiler:?}").contains("StaticConfigCompiler"));
    assert!(format!("{:?}", crate::FnCompiler(())).contains("FnCompiler"));

    let options = crate::WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: compiler.clone(),
        config: open_compute_core::config::RuntimeConfig::default(),
        clock: Arc::new(open_compute_core::SystemClock),
        jitter: Arc::new(crate::OsJitter),
        redactor: Redactor::new(),
        lease_path: None,
    };
    assert!(format!("{options:?}").contains("WorkerdSupervisorOptions"));
    let supervisor = crate::WorkerdSupervisor::new(options, Vec::new(), Vec::new(), Vec::new());
    assert!(format!("{supervisor:?}").contains("WorkerdSupervisor"));
    supervisor.shutdown().await;

    let defaults = crate::WorkerdSupervisor::with_defaults(
        runtime,
        compiler,
        open_compute_core::config::RuntimeConfig::default(),
        Redactor::new(),
    );
    assert!(format!("{defaults:?}").contains("WorkerdSupervisor"));
    defaults.shutdown().await;
}
