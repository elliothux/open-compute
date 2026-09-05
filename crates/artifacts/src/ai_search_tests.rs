use super::*;
use crate::{MapEnv, MockS3, resolve_s3_credentials_with};
use std::os::unix::fs::PermissionsExt as _;

#[tokio::test]
async fn object_store_is_content_addressed_idempotent_and_exactly_deleted() {
    let mock = MockS3::spawn("bucket").await;
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "bucket".to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    let store = AiSearchObjectStore::new(
        ObjectBackend::connect_s3(&config, &credentials, 4 * 1024 * 1024).unwrap(),
    );
    let account = AccountId::generate();
    let instance = ResourceId::generate();
    let body = b"AI Search source";
    let reference = AiSearchObjectRef::new(
        account,
        instance,
        Sha256::digest(body).into(),
        body.len() as u64,
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::write(&source, body).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let key = store.put_file(&reference, &source).await.unwrap();
    assert_eq!(store.put_file(&reference, &source).await.unwrap(), key);
    let download = store.download(&reference, &key).await.unwrap();
    assert_eq!(download.size, body.len() as u64);
    assert_eq!(
        download.body.collect().await.unwrap().into_bytes().as_ref(),
        body
    );
    let wrong_key = format!("{key}-neighbor");
    assert_eq!(
        store
            .verify(&reference, &wrong_key)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    store.delete_exact(&reference, &key).await.unwrap();
    assert!(store.verify(&reference, &key).await.is_err());
}

#[test]
fn object_identity_enforces_size_and_canonical_layout() {
    let account = AccountId::generate();
    let instance = ResourceId::generate();
    assert!(AiSearchObjectRef::new(account, instance, [0; 32], 0).is_err());
    let reference = AiSearchObjectRef::new(account, instance, [0xab; 32], 1).unwrap();
    assert_eq!(
        reference.object_key("system/"),
        format!(
            "system/ai-search/v1/{account}/{instance}/objects/sha256/ab/{}",
            "ab".repeat(32)
        )
    );
}
