use super::*;

#[test]
fn identity_bootstrap_rejects_corrupt_existing_authority_rows() {
    enum Corruption {
        MissingInstance,
        Instance(String),
        Created(i64),
    }
    for corruption in [
        Corruption::MissingInstance,
        Corruption::Instance("bad-instance".to_owned()),
        Corruption::Created(-1),
    ] {
        let (_tmp, root) = unique_root();
        let config = storage_config(&root);
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let fingerprint = storage.crypto().fingerprint_key_id().to_owned();
        drop(storage);
        let db_path = root.join("control.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        match corruption {
            Corruption::MissingInstance => {
                conn.execute("DELETE FROM instance_identity", []).unwrap();
            }
            Corruption::Instance(value) => {
                conn.execute("UPDATE instance_identity SET instance_id = ?1", [value])
                    .unwrap();
            }
            Corruption::Created(value) => {
                conn.execute("UPDATE instance_identity SET created_at_ms = ?1", [value])
                    .unwrap();
            }
        }
        drop(conn);
        let db = crate::control_db::ControlDb::open(&db_path, 5_000).unwrap();
        assert!(crate::identity::bootstrap(&db, &SystemClock, &fingerprint).is_err());
    }
}
