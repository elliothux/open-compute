use super::*;
use rustix::fs::{FlockOperation, flock};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Instant;

#[test]
fn wait_until_quiescent_holds_until_data_lock_release() {
    let temp = TempDir::new().unwrap();
    let registry = scratch_registry(&temp);
    let (config, record) = register_config(&temp, &registry);
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    let lock_path = loaded.config.data.data_lock_path();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .unwrap();
    flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        drop(lock);
    });
    let start = Instant::now();
    wait_until_instance_quiescent(
        &record,
        &lock_path,
        Some(&temp.path().join("runtime")),
        &FakeServiceManager::default(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(200));
    release.join().unwrap();
}
