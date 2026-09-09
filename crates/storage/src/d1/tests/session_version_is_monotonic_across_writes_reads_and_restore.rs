use super::*;

#[test]
fn session_version_is_monotonic_across_writes_reads_and_restore() {
    let fixture = fixture();
    assert_eq!(fixture.engine.session_version().unwrap(), 0);
    fixture
        .engine
        .query(&statement("SELECT 1", vec![]), limits())
        .unwrap();
    assert_eq!(fixture.engine.session_version().unwrap(), 0);
    fixture
        .engine
        .exec(
            "CREATE TABLE versions(id INTEGER PRIMARY KEY, value TEXT)",
            limits(),
        )
        .unwrap();
    let after_create = fixture.engine.session_version().unwrap();
    assert!(after_create >= 1);
    fixture
        .engine
        .query(
            &statement("INSERT INTO versions(value) VALUES ('one')", vec![]),
            limits(),
        )
        .unwrap();
    let after_insert = fixture.engine.session_version().unwrap();
    assert!(after_insert > after_create);
    fixture
        .engine
        .query(&statement("SELECT value FROM versions", vec![]), limits())
        .unwrap();
    assert_eq!(fixture.engine.session_version().unwrap(), after_insert);
    let snapshot = fixture._temp.path().join("snapshot.sqlite");
    fixture.engine.online_backup(&snapshot).unwrap();
    let restored = D1Engine::restore_as_new(
        &snapshot,
        &fixture._temp.path().join("restored.sqlite"),
        AccountId::generate(),
        ResourceId::generate(),
        30,
        256 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(restored.session_version().unwrap(), after_insert);
    restored
        .query(
            &statement("INSERT INTO versions(value) VALUES ('two')", vec![]),
            limits(),
        )
        .unwrap();
    assert!(restored.session_version().unwrap() > after_insert);
    let key = SecretBytes::new(vec![5_u8; 32]);
    let crypto = SecretCrypto::new(&key, &master_key::fingerprint_for_test(key.expose())).unwrap();
    let current = fixture.engine.session_version().unwrap();
    let future = crypto
        .seal_d1_bookmark(fixture.account, fixture.resource, current + 1)
        .unwrap();
    assert_eq!(
        crypto
            .open_d1_bookmark(fixture.account, fixture.resource, &future)
            .unwrap(),
        current + 1
    );
    assert!(
        crypto
            .open_d1_bookmark(fixture.account, fixture.resource, &future)
            .unwrap()
            > fixture.engine.session_version().unwrap()
    );
}
