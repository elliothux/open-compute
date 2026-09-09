use super::*;

#[test]
fn progress_handler_distinguishes_vm_budget_and_wall_timeout() {
    let fixture = fixture();
    let expensive = statement(
        "WITH RECURSIVE count(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM count WHERE x < 100000) SELECT sum(x) FROM count",
        vec![],
    );
    let vm_limited = D1QueryLimits {
        max_vm_steps: 1_000,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&expensive, vm_limited)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let timed_out = D1QueryLimits {
        timeout: std::time::Duration::ZERO,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&expensive, timed_out)
            .unwrap_err()
            .code(),
        ErrorCode::D1Timeout
    );
    assert_eq!(
        fixture
            .engine
            .batch(&[expensive], timed_out)
            .unwrap_err()
            .code(),
        ErrorCode::D1Timeout
    );
}
