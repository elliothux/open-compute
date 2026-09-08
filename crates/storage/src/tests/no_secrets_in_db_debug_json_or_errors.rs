use super::*;

#[test]
fn no_secrets_in_db_debug_json_or_errors() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let key_file = fs::read_to_string(&config.master_key_file).unwrap();
    let secret_body = key_file.trim().strip_prefix("ocmk1:").unwrap();
    let debug = format!("{storage:?}");
    let json_lock = fs::read_to_string(root.join("platform.lock")).unwrap();
    let db_bytes = storage.db().dump_bytes().unwrap();
    let db_text = String::from_utf8_lossy(&db_bytes);
    let err = open_compute_core::PlatformError::new(
        ErrorCode::MasterKeyMismatch,
        "master key fingerprint mismatch",
    );
    let err_json = serde_json::to_string(&err).unwrap();
    for hay in [
        debug.as_str(),
        json_lock.as_str(),
        db_text.as_ref(),
        err_json.as_str(),
    ] {
        assert!(!hay.contains(secret_body), "leaked key material");
        assert!(!hay.contains("super-secret-value"));
    }
    let raw = fs::read(root.join("control.sqlite")).unwrap();
    assert!(
        !raw.windows(secret_body.len())
            .any(|w| w == secret_body.as_bytes())
    );
}
