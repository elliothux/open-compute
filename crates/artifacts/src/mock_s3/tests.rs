use super::*;
use tokio::net::TcpStream;

#[test]
fn request_parsing_helpers_cover_valid_and_invalid_inputs() {
    assert_eq!(find_headers_end(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
    assert_eq!(find_headers_end(b"incomplete"), None);
    assert_eq!(split_uri("/path?x=1"), ("/path".into(), "x=1".into()));
    assert_eq!(split_uri("/path"), ("/path".into(), String::new()));
    assert_eq!(
        query_param("x=1&prefix=a%2Fb", "prefix"),
        Some("a/b".into())
    );
    assert_eq!(query_param("x&y=2", "prefix"), None);
    assert_eq!(percent_decode("a%2Fb%zz"), "a/b%zz");
}

#[tokio::test]
async fn mock_debug_state_and_raw_protocol_paths() {
    let mock = MockS3::spawn("bucket").await;
    assert!(format!("{mock:?}").contains(&mock.endpoint));
    assert!(mock.keys().is_empty());
    mock.put_raw("prefix/key", b"body".to_vec());
    assert_eq!(mock.keys(), vec!["prefix/key".to_string()]);
    mock.corrupt_body("missing");
    mock.set_get_chunking(0, Duration::ZERO);
    mock.set_omit_last_modified(true);
    assert_eq!(mock.artifact_gets(), 0);
    mock.clear_recorded();

    async fn raw(endpoint: &str, request: &[u8]) -> String {
        let address = endpoint.strip_prefix("http://").unwrap();
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(request).await.unwrap();
        let mut response = vec![0_u8; 1024];
        let n = stream.read(&mut response).await.unwrap();
        String::from_utf8_lossy(&response[..n]).into_owned()
    }
    assert!(
        raw(
            &mock.endpoint,
            b"POST /bucket/key HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
        )
        .await
        .starts_with("HTTP/1.1 405")
    );
    assert!(
        raw(
            &mock.endpoint,
            b"HEAD /wrong/key HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
        )
        .await
        .starts_with("HTTP/1.1 404")
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move { TcpStream::connect(address).await.unwrap() });
    let (mut server, _) = listener.accept().await.unwrap();
    let _client = peer.await.unwrap();
    let decoded = read_aws_chunked(&mut server, b"3\r\nabc\r\n0\r\n\r\n".to_vec(), 3)
        .await
        .unwrap();
    assert_eq!(decoded, b"abc");
}
