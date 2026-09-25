use super::*;

#[test]
fn scheduler_identity_is_bound_to_control_authority() {
    let temp = tempfile::tempdir().unwrap();
    let a =
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("a")), &SystemClock).unwrap();
    let b =
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("b")), &SystemClock).unwrap();
    let scheduler_path = a.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1, a.identity().instance_id).unwrap());
    let control_a = a.data_dir().control_db_path();
    let control_b = b.data_dir().control_db_path();

    crate::inspect_p23_cross_database(&control_a, &scheduler_path, 5_000).unwrap();
    crate::scheduler::inspect_workflow_databases(&control_a, &scheduler_path, 5_000, 10).unwrap();
    assert_eq!(
        crate::inspect_p23_cross_database(&control_b, &scheduler_path, 5_000)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerCorrupt
    );
    assert_eq!(
        crate::scheduler::inspect_workflow_databases(&control_b, &scheduler_path, 5_000, 10)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        crate::SchedulerStore::open(&scheduler_path, 5_000, 2, b.identity().instance_id)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerCorrupt
    );
}
