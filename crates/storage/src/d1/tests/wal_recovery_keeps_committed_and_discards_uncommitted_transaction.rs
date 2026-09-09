use super::*;

#[test]
fn wal_recovery_keeps_committed_and_discards_uncommitted_transaction() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE TABLE recovery(value TEXT)", limits())
        .unwrap();
    {
        let connection = fixture.engine.open().unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        connection
            .execute("INSERT INTO recovery(value) VALUES ('uncommitted')", [])
            .unwrap();
    }
    fixture
        .engine
        .exec("INSERT INTO recovery(value) VALUES ('committed')", limits())
        .unwrap();
    drop(fixture.engine.open().unwrap());
    let result = fixture
        .engine
        .query(&statement("SELECT value FROM recovery", vec![]), limits())
        .unwrap();
    assert_eq!(
        result.rows,
        vec![vec![D1Value::Text("committed".to_owned())]]
    );
}
