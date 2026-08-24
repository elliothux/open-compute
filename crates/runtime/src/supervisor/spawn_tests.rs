use super::*;

#[test]
fn private_failure_and_process_group_helpers_are_fail_closed() {
    let failure = SpawnFailure::without_child(PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "test failure",
    ));
    assert_eq!(failure.error.code(), ErrorCode::RuntimeInvalid);
    assert!(failure.pid.is_none());
    assert!(failure.pgid.is_none());
    assert!(failure.completion.is_none());

    assert_eq!(read_pgid(0).unwrap_err().code(), ErrorCode::RuntimeInvalid);
    assert_eq!(
        read_pgid(i32::MAX).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    set_spawn_fail_point("unknown");
    assert!(fail_point() == FailPoint::None);
    let _ = last_spawned_pid();
}
