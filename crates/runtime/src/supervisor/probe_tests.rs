use super::*;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

#[tokio::test]
async fn fragmented_valid_204_passes() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = vec![0u8; 512];
        let _ = sock.read(&mut req).await;
        let body = b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        for byte in body {
            sock.write_all(&[*byte]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    probe_once(&port, READY_PATH).await.unwrap();
}

#[tokio::test]
async fn late_fragment_body_on_204_fails() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = vec![0u8; 512];
        let _ = sock.read(&mut req).await;
        sock.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = sock.write_all(b"illegal-body").await;
    });
    assert!(probe_once(&port, READY_PATH).await.is_err());
}

#[tokio::test]
async fn oversized_and_malformed_fail() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = vec![0u8; 512];
        let _ = sock.read(&mut req).await;
        let huge = vec![b'X'; MAX_RESPONSE + 8];
        let _ = sock.write_all(&huge).await;
    });
    assert!(probe_once(&port, READY_PATH).await.is_err());

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = vec![0u8; 512];
        let _ = sock.read(&mut req).await;
        let _ = sock.write_all(b"NOTHTTP\r\n\r\n").await;
    });
    assert!(probe_once(&port, READY_PATH).await.is_err());
}

#[test]
fn unit_parser_fragment_and_bounds() {
    assert!(
        parse_http_response_for_test(b"HTTP/1.1 204")
            .unwrap()
            .is_none()
    );
    let full = b"HTTP/1.1 204 No Content\r\n\r\n";
    assert_eq!(parse_http_response_for_test(full).unwrap(), Some(204));
    assert!(
        try_parse_http(full, false).unwrap().is_none(),
        "204 without EOF must wait for Connection: close"
    );
    let huge = vec![b'A'; MAX_HEADER + 1];
    assert!(parse_http_response_for_test(&huge).is_err());
    assert!(
        parse_http_response_for_test(
            b"HTTP/1.1 204 No Content\r\nTransfer-Encoding: chunked\r\n\r\n"
        )
        .is_err()
    );
    assert!(
        parse_http_response_for_test(b"HTTP/1.1 204 No Content\r\nContent-Length: 1\r\n\r\n")
            .is_err()
    );
    assert!(
        parse_http_response_for_test(
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n"
        )
        .is_err()
    );
    assert!(parse_http_response_for_test(b"HTTP/1.1 204 No Content\r\n\r\nX").is_err());
    assert_eq!(
        parse_http_response_for_test(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .unwrap(),
        Some(204)
    );

    assert_eq!(
        parse_http_response_for_test(b"HTTP/1.1 503 Unavailable\r\n\r\n").unwrap(),
        Some(503)
    );
    assert!(!empty_204_complete(b"HTTP/1.1 503 Unavailable\r\n\r\n").unwrap());
    assert!(parse_http_response_for_test(b"\xff\r\n\r\n").is_err());
    assert!(parse_http_response_for_test(b"HTTP/1.1 nope\r\n\r\n").is_err());
    assert!(parse_http_response_for_test(b"HTTP/1.1 200 OK\r\nbad-header\r\n\r\n").is_err());
    assert!(
        parse_http_response_for_test(b"HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\n").is_err()
    );
    assert!(
        parse_http_response_for_test(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_RESPONSE + 1
            )
            .as_bytes()
        )
        .is_err()
    );
    assert!(
        try_parse_http(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nx", false)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        parse_http_response_for_test(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx"
        )
        .unwrap(),
        Some(200)
    );
    let mut overlong_header = b"HTTP/1.1 200 OK\r\nX: ".to_vec();
    overlong_header.extend(vec![b'a'; MAX_HEADER]);
    overlong_header.extend_from_slice(b"\r\n\r\n");
    assert!(parse_http_response_for_test(&overlong_header).is_err());
}

#[tokio::test]
async fn wrappers_reject_connect_failure_and_deadline() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let unused = dir.path().join("unused.sock");
    let listener = UnixListener::bind(&unused).unwrap();
    drop(listener);
    assert!(
        probe_ready(&unused, Duration::from_millis(20))
            .await
            .is_err()
    );

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (_sock, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    assert!(probe_ready(&port, Duration::from_millis(10)).await.is_err());
}

#[tokio::test]
async fn eof_empty_rejected_status_and_read_deadline_are_typed() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        let _ = sock.read(&mut request).await;
    });
    assert!(probe_once(&port, READY_PATH).await.is_err());

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 512];
        let _ = sock.read(&mut request).await;
        sock.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    assert!(probe_once(&port, READY_PATH).await.is_err());

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let port = dir.path().join("probe.sock");
    let listener = UnixListener::bind(&port).unwrap();
    let client = tokio::spawn(async move { UnixStream::connect(port).await.unwrap() });
    let (mut server, _) = listener.accept().await.unwrap();
    let _client = client.await.unwrap();
    assert!(
        read_http_response(&mut server, Duration::from_millis(5))
            .await
            .is_err()
    );
}
