use super::*;

#[test]
fn identity_bootstrap_rejects_corrupt_existing_authority_rows() {
    enum Corruption {
        Platform(Vec<u8>),
        Created(Vec<u8>),
        Account(String),
    }
    for corruption in [
        Corruption::Platform(b"bad-platform".to_vec()),
        Corruption::Platform(vec![0xff]),
        Corruption::Created(b"not-a-number".to_vec()),
        Corruption::Account("bad-account".to_owned()),
    ] {
        let (_tmp, root) = unique_root();
        let config = storage_config(&root);
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let fingerprint = storage.crypto().fingerprint_key_id().to_owned();
        drop(storage);
        let db_path = root.join("control.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        match corruption {
            Corruption::Platform(value) => {
                conn.execute(
                    "UPDATE platform_meta SET value = ?1 WHERE key = 'platform_id'",
                    [value],
                )
                .unwrap();
            }
            Corruption::Created(value) => {
                conn.execute(
                    "UPDATE platform_meta SET value = ?1 WHERE key = 'created_at_ms'",
                    [value],
                )
                .unwrap();
            }
            Corruption::Account(value) => {
                conn.execute(
                    "UPDATE accounts SET id = ?1 WHERE name = 'default'",
                    [value],
                )
                .unwrap();
            }
        }
        drop(conn);
        let db = crate::control_db::ControlDb::open(&db_path, 5_000).unwrap();
        assert!(crate::identity::bootstrap(&db, &SystemClock, &fingerprint).is_err());
    }
}
