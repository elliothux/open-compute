use super::*;

#[test]
fn corrupt_database_is_local_and_does_not_block_another_file() {
    let corrupt = fixture();
    let healthy = fixture();
    healthy
        .engine
        .exec("CREATE TABLE healthy(value INTEGER)", limits())
        .unwrap();
    drop(corrupt.engine.open().unwrap());
    std::fs::write(&corrupt.engine.path, b"not a sqlite database").unwrap();
    assert_eq!(
        corrupt.engine.quick_check().unwrap_err().code(),
        ErrorCode::D1DatabaseCorrupt,
    );
    healthy
        .engine
        .query(&statement("SELECT count(*) FROM healthy", vec![]), limits())
        .unwrap();
}
