use super::*;

#[test]
fn create_crud_types_and_raw_column_order() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, enabled INTEGER, data BLOB)",
            limits(),
        )
        .unwrap();
    let insert = fixture
        .engine
        .query(
            &statement(
                "INSERT INTO users(name, enabled, data) VALUES (?1, ?2, ?3) RETURNING id",
                vec![
                    D1Value::Text("Ada".to_owned()),
                    D1Value::Integer(1),
                    D1Value::Blob(vec![0, 1, 255]),
                ],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(insert.rows, vec![vec![D1Value::Integer(1)]]);
    assert!(insert.meta.changed_db);
    assert_eq!(insert.meta.changes, 1);
    let selected = fixture
        .engine
        .query(
            &statement(
                "SELECT name AS duplicate, enabled AS duplicate, data, NULL, 1.5 FROM users",
                vec![],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(selected.columns[0..2], ["duplicate", "duplicate"]);
    assert_eq!(selected.rows[0][2], D1Value::Blob(vec![0, 1, 255]));
    assert_eq!(selected.rows[0][3], D1Value::Null);
    assert_eq!(selected.rows[0][4], D1Value::Real(1.5));
    assert_eq!(selected.meta.rows_read, 1);
    assert_eq!(selected.meta.changes, 0);
    assert_eq!(selected.meta.rows_written, 0);
    assert!(!selected.meta.changed_db);
}
