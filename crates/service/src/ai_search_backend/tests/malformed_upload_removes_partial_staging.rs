use super::*;

#[tokio::test]
async fn malformed_upload_removes_partial_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("upload");
    let body = Body::from(Bytes::from_static(&[0, 1, 0, 0]));
    assert_eq!(
        stage_upload(body, path.clone()).await.unwrap_err().code(),
        ErrorCode::BindingProtocolError
    );
    assert!(!path.exists());
}
