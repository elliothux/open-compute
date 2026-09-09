use super::*;

#[test]
fn online_backup_and_restore_rewrite_identity_but_keep_tenant_data() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE data(value TEXT); INSERT INTO data VALUES ('kept')",
            limits(),
        )
        .unwrap();
    let backup = fixture._temp.path().join("backup.sqlite");
    fixture.engine.online_backup(&backup).unwrap();
    let new_account = AccountId::generate();
    let new_resource = ResourceId::generate();
    let restored_path = fixture._temp.path().join("restored.sqlite");
    let restored = D1Engine::restore_as_new(
        &backup,
        &restored_path,
        new_account,
        new_resource,
        200,
        64 * 1024 * 1024,
    )
    .unwrap();
    restored.verify_identity().unwrap();
    let rows = restored
        .query(&statement("SELECT value FROM data", vec![]), limits())
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Text("kept".to_owned())]]);
    assert_ne!(fixture.account, new_account);
    assert_ne!(fixture.resource, new_resource);
}
