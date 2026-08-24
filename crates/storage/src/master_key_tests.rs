use super::*;
use std::fs;

#[test]
fn generated_key_persistence_covers_success_collision_and_install_failure() {
    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("master.key");
    let temp_path = dir.path().join("master.tmp");
    let encoded = encode_key(&[7; KEY_LEN]);

    persist_generated_key(dir.path(), &final_path, &temp_path, encoded.as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(&final_path).unwrap(), encoded);
    assert!(!temp_path.exists());

    let collision_temp = dir.path().join("collision.tmp");
    let error = persist_generated_key(dir.path(), &final_path, &collision_temp, encoded.as_bytes())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(!collision_temp.exists());

    let failed_temp = dir.path().join("failed.tmp");
    let missing_final = dir.path().join("missing/master.key");
    let error = persist_generated_key(dir.path(), &missing_final, &failed_temp, encoded.as_bytes())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(failed_temp.exists());
}

#[test]
fn generated_key_rejects_root_and_non_directory_parent() {
    assert_eq!(
        generate_and_create(Path::new("/")).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("not-a-directory");
    fs::write(&parent, b"file").unwrap();
    assert_eq!(
        generate_and_create(&parent.join("master.key"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}
