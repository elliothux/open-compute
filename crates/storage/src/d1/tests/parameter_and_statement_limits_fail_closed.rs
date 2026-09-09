use super::*;

#[test]
fn parameter_and_statement_limits_fail_closed() {
    let fixture = fixture();
    let mismatch = fixture
        .engine
        .query(&statement("SELECT ?1", vec![]), limits())
        .unwrap_err();
    assert_eq!(mismatch.code(), ErrorCode::D1ParameterMismatch);
    let multiple = fixture
        .engine
        .query(&statement("SELECT 1; SELECT 2", vec![]), limits())
        .unwrap_err();
    assert_eq!(multiple.code(), ErrorCode::D1SqlInvalid);
    let huge = "x".repeat(D1_MAX_SQL_BYTES + 1);
    assert_eq!(
        fixture
            .engine
            .query(&statement(&huge, vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid,
    );
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT ?1", vec![D1Value::Real(f64::NAN)]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError,
    );
}
