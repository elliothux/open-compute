use super::*;

#[tokio::test]
async fn upload_frame_is_streamed_to_private_exact_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("upload");
    let metadata = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "instance": "docs",
        "name": "guide.txt",
        "contentType": "text/plain",
        "options": {"metadata": {"language": "en"}},
    }))
    .unwrap();
    let mut frame = u32::try_from(metadata.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(&metadata);
    frame.extend_from_slice(b"hello streamed world");
    let pieces = frame
        .chunks(3)
        .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let body = Body::from_stream(stream::iter(pieces));
    let staged = stage_upload(body, path.clone(), MAX_UPLOAD_BYTES as u64)
        .await
        .unwrap();
    assert_eq!(staged.header.instance.as_deref(), Some("docs"));
    assert_eq!(staged.header.name, "guide.txt");
    assert_eq!(staged.size, 20);
    let expected_digest: [u8; 32] = Sha256::digest(b"hello streamed world").into();
    assert_eq!(staged.digest, expected_digest);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello streamed world");
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
