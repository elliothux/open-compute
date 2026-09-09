use super::*;

#[test]
fn lock_released_after_failed_bootstrap() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let err = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::BeforeExecution),
    )
    .expect_err("fault");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    PlatformStorage::bootstrap(&config, &SystemClock).expect("retry after failed bootstrap");
}
