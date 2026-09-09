use super::*;

#[test]
fn separate_database_files_are_isolated() {
    let first = fixture();
    let second = fixture();
    first
        .engine
        .exec("CREATE TABLE only_first(value)", limits())
        .unwrap();
    assert_eq!(
        second
            .engine
            .query(&statement("SELECT * FROM only_first", vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid,
    );
}
