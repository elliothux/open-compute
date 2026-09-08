use super::*;

#[test]
fn sqlite_and_materialization_limits_enforce_exact_boundaries() {
    let fixture = fixture();
    let mut exact_sql = "SELECT 1".to_owned();
    exact_sql.push_str(&" ".repeat(D1_MAX_SQL_BYTES - exact_sql.len()));
    fixture
        .engine
        .query(&statement(&exact_sql, vec![]), limits())
        .unwrap();

    let parameter_sql = format!(
        "SELECT {}",
        (1..=D1_MAX_BOUND_PARAMS)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    fixture
        .engine
        .query(
            &statement(
                &parameter_sql,
                vec![D1Value::Integer(1); D1_MAX_BOUND_PARAMS],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT 1", vec![D1Value::Null; D1_MAX_BOUND_PARAMS + 1]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    let too_many_columns = format!("SELECT {}", vec!["1"; D1_MAX_COLUMNS + 1].join(","));
    assert_eq!(
        fixture
            .engine
            .query(&statement(&too_many_columns, vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    fixture
        .engine
        .query(
            &statement("SELECT 'abc' LIKE ?1", vec![D1Value::Text("x".repeat(50))]),
            limits(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT 'abc' LIKE ?1", vec![D1Value::Text("x".repeat(51))]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let function_args = |count: usize| format!("SELECT printf({})", vec!["'x'"; count].join(","));
    fixture
        .engine
        .query(&statement(&function_args(32), vec![]), limits())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(&statement(&function_args(33), vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    let one_row = D1QueryLimits {
        max_result_rows: 1,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&statement("SELECT 1 UNION ALL SELECT 2", vec![]), one_row,)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let one_byte = D1QueryLimits {
        max_result_bytes: 1,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&statement("SELECT 'ab'", vec![]), one_byte)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    fixture
        .engine
        .exec("CREATE TABLE bounded_write(value TEXT)", limits())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement(
                    "INSERT INTO bounded_write VALUES ('x') RETURNING 'ab'",
                    vec![]
                ),
                one_byte,
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT count(*) FROM bounded_write", vec![]),
                limits(),
            )
            .unwrap()
            .rows,
        vec![vec![D1Value::Integer(0)]]
    );
}
