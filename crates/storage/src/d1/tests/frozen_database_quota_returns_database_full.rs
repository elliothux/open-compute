use super::*;

#[test]
fn frozen_database_quota_returns_database_full() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE quota_probe(id INTEGER PRIMARY KEY, payload BLOB)",
            limits(),
        )
        .unwrap();
    let mut successful = 0_u32;
    let terminal = loop {
        match fixture.engine.query(
            &statement(
                "INSERT INTO quota_probe(payload) VALUES (zeroblob(1000000))",
                vec![],
            ),
            limits(),
        ) {
            Ok(_) => successful += 1,
            Err(error) => break error,
        }
        assert!(successful < 100, "quota did not stop bounded growth");
    };
    assert_eq!(terminal.code(), ErrorCode::D1DatabaseFull);
    assert!(successful >= 50);
}
