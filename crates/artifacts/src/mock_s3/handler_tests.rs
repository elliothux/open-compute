use super::*;
use std::cell::Cell;

#[tokio::test]
async fn multipart_provider_checksums_leave_the_application_scheduler_responsive() {
    let mock = MockS3::spawn("bucket").await;
    mock.state.lock().unwrap().uploads.insert(
        "upload".to_owned(),
        MultipartUpload {
            key: "object".to_owned(),
            parts: Default::default(),
            metadata: Default::default(),
            response_headers: Default::default(),
            storage_class: "STANDARD".to_owned(),
            ssec_key_md5: None,
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (peer, accepted) = tokio::join!(
        tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
        listener.accept()
    );
    let mut peer = peer.unwrap();
    let (mut server, _) = accepted.unwrap();
    server.writable().await.unwrap();
    let body = vec![0x5a; 8 * 1024 * 1024];
    let expected_etag = hex::encode(md5::Md5::digest(&body));
    let mut request = ParsedRequest {
        method: "PUT".to_owned(),
        path: "/bucket/object".to_owned(),
        query: "partNumber=1&uploadId=upload".to_owned(),
        headers: Default::default(),
        body,
        fault: Fault::None,
    };
    let complete = Cell::new(false);
    let application_turns = Cell::new(0_u64);
    let provider = async {
        let result = handle_special_routes(&mut server, &mock.state, &mut request).await;
        complete.set(true);
        result.unwrap()
    };
    let application = async {
        while !complete.get() {
            application_turns.set(application_turns.get() + 1);
            tokio::task::yield_now().await;
        }
    };
    let (result, ()) = tokio::join!(biased; provider, application);
    assert!(matches!(result, SpecialRoute::Handled));
    let mut response = [0_u8; 1024];
    let count = peer.read(&mut response).await.unwrap();
    assert!(response[..count].starts_with(b"HTTP/1.1 200"));
    {
        let state = mock.state.lock().unwrap();
        let (etag, stored) = &state.uploads["upload"].parts[&1];
        assert_eq!(etag, &expected_etag);
        assert_eq!(stored.len(), 8 * 1024 * 1024);
        assert!(stored.iter().all(|byte| *byte == 0x5a));
    }
    assert!(
        application_turns.get() > 0,
        "the in-process provider blocked the application scheduler while hashing a part"
    );
}
