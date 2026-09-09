use super::*;

pub(super) async fn run() {
    let workerd = required_workerd();
    let repo = repo_root();
    let lock_path = repo.join("packages/runtime/workerd.lock.json");
    let (lock, _) = load_runtime_lock(&lock_path).expect("lock");
    verify_workerd(&lock, &workerd);
    let staging_before = staging_directories();

    let s3 = MockS3::spawn("open-compute").await;
    run_round(1, &s3, &lock).await;
    assert_eq!(
        staging_directories(),
        staging_before,
        "P0.1 Gate leaked a macOS executable staging directory"
    );
    drop(s3);
}
