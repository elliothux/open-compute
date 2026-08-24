//! In-process HTTP S3-compatible test server.

#![allow(missing_docs)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    None,
    Auth,
    Permission,
    ServerError,
    Timeout,
    CorruptMetadata,
    CorruptBody,
    DeleteFail,
    NotFound,
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    #[allow(dead_code)]
    pub query: String,
    pub has_authorization: bool,
    pub authorization: Option<String>,
}

#[derive(Clone)]
pub(crate) struct StoredObject {
    pub body: Vec<u8>,
    pub sha256: String,
    #[allow(dead_code)]
    pub modified: SystemTime,
}

#[derive(Clone)]
pub struct MockS3 {
    pub endpoint: String,
    state: Arc<Mutex<Inner>>,
    shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl std::fmt::Debug for MockS3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockS3")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

struct Inner {
    bucket: String,
    objects: HashMap<String, StoredObject>,
    fault: Fault,
    recorded: Vec<Recorded>,
    get_chunk_size: usize,
    get_chunk_delay: Duration,
    omit_last_modified: bool,
    head_barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl MockS3 {
    pub async fn spawn(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(Inner {
            bucket: bucket.to_string(),
            objects: HashMap::new(),
            fault: Fault::None,
            recorded: Vec::new(),
            get_chunk_size: usize::MAX,
            get_chunk_delay: Duration::ZERO,
            omit_last_modified: false,
            head_barrier: None,
        }));
        let (tx, mut rx) = oneshot::channel();
        let state_clone = Arc::clone(&state);
        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    acc = listener.accept() => {
                        match acc {
                            Ok((stream, _)) => {
                                let state = Arc::clone(&state_clone);
                                tokio::spawn(async move {
                                    let _ = handle_conn(stream, state).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
        Self {
            endpoint: format!("http://{addr}"),
            state,
            shutdown: Arc::new(Mutex::new(Some(tx))),
            join: Arc::new(Mutex::new(Some(join))),
        }
    }

    pub fn set_fault(&self, fault: Fault) {
        self.state.lock().expect("lock").fault = fault;
    }

    pub fn synchronize_next_heads(&self, participants: usize) {
        self.state.lock().expect("lock").head_barrier =
            Some(Arc::new(tokio::sync::Barrier::new(participants)));
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.state.lock().expect("lock").recorded.clone()
    }

    /// Drop recorded requests so the next observer sees only subsequent traffic.
    pub fn clear_recorded(&self) {
        self.state.lock().expect("lock").recorded.clear();
    }

    /// Snapshot of object keys currently stored.
    pub fn keys(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("lock")
            .objects
            .keys()
            .cloned()
            .collect()
    }

    pub fn object_count(&self) -> usize {
        self.state.lock().expect("lock").objects.len()
    }

    pub fn put_raw(&self, key: &str, body: Vec<u8>) {
        let sha256 = hex::encode(Sha256::digest(&body));
        self.state.lock().expect("lock").objects.insert(
            key.to_string(),
            StoredObject {
                body,
                sha256,
                modified: SystemTime::now(),
            },
        );
    }

    pub fn corrupt_body(&self, key: &str) {
        if let Some(obj) = self.state.lock().expect("lock").objects.get_mut(key) {
            obj.body.push(0xff);
        }
    }

    pub fn set_get_chunking(&self, chunk_size: usize, delay: Duration) {
        let mut g = self.state.lock().expect("lock");
        g.get_chunk_size = chunk_size.max(1);
        g.get_chunk_delay = delay;
    }

    pub fn set_omit_last_modified(&self, omit: bool) {
        self.state.lock().expect("lock").omit_last_modified = omit;
    }

    pub fn artifact_gets(&self) -> usize {
        self.recorded()
            .iter()
            .filter(|r| r.method == "GET" && r.path.contains("/artifacts/v1/sha256/"))
            .count()
    }
}

impl Drop for MockS3 {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.lock().expect("lock").take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.lock().expect("lock").take() {
            join.abort();
        }
    }
}

async fn handle_conn(
    mut stream: tokio::net::TcpStream,
    state: Arc<Mutex<Inner>>,
) -> Result<(), std::io::Error> {
    loop {
        let mut buf = Vec::new();
        let mut tmp = [0_u8; 1024];
        let header_end;
        loop {
            let n = match stream.read(&mut tmp).await {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::ConnectionReset => return Ok(()),
                Err(e) => return Err(e),
            };
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_headers_end(&buf) {
                header_end = pos;
                break;
            }
            if buf.len() > 1024 * 1024 {
                write_status(&mut stream, 400, "Bad Request", b"").await?;
                return Ok(());
            }
        }
        let header_bytes = buf[..header_end].to_vec();
        let extra = buf[header_end..].to_vec();
        let header_text = String::from_utf8_lossy(&header_bytes);
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let uri = parts.next().unwrap_or("");
        let (path, query) = split_uri(uri);
        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let authorization = headers.get("authorization").cloned();
        let has_authorization = authorization.is_some();
        {
            let mut g = state.lock().expect("lock");
            g.recorded.push(Recorded {
                method: method.clone(),
                path: path.clone(),
                query: query.clone(),
                has_authorization,
                authorization,
            });
        }
        let content_length = headers
            .get("content-length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let decoded = headers
            .get("x-amz-decoded-content-length")
            .and_then(|v| v.parse::<usize>().ok());
        let extra_len = extra.len();
        let mut body = extra;
        if let Some(decoded) = decoded {
            body = read_aws_chunked(&mut stream, body, decoded).await?;
        } else if body.len() < content_length {
            body.resize(content_length, 0);
            stream
                .read_exact(&mut body[extra_len.min(content_length)..])
                .await?;
            body.truncate(content_length);
        } else {
            body.truncate(content_length);
        }

        let fault = state.lock().expect("lock").fault;
        let apply_fault = method != "DELETE" || fault == Fault::DeleteFail;
        if apply_fault {
            match fault {
                Fault::Auth => {
                    write_s3_err(&mut stream, 403, "InvalidAccessKeyId").await?;
                    continue;
                }
                Fault::Permission => {
                    write_s3_err(&mut stream, 403, "AccessDenied").await?;
                    continue;
                }
                Fault::ServerError => {
                    write_s3_err(&mut stream, 500, "InternalError").await?;
                    continue;
                }
                Fault::Timeout => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    write_s3_err(&mut stream, 500, "InternalError").await?;
                    continue;
                }
                Fault::NotFound => {
                    write_s3_err(&mut stream, 404, "NoSuchKey").await?;
                    continue;
                }
                _ => {}
            }
        }

        let bucket = state.lock().expect("lock").bucket.clone();
        let prefix = format!("/{bucket}/");
        if method == "HEAD" && (path == format!("/{bucket}") || path == prefix) {
            write_status(&mut stream, 200, "OK", b"").await?;
            continue;
        }

        if method == "GET"
            && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
            && (query.contains("list-type=2") || query.contains("prefix="))
        {
            let list_prefix = query_param(&query, "prefix").unwrap_or_default();
            let xml = list_xml(&state, &bucket, &list_prefix);
            write_xml(&mut stream, 200, &xml).await?;
            continue;
        }

        let key = match path.strip_prefix(&prefix) {
            Some(k) => percent_decode(k),
            None => {
                write_s3_err(&mut stream, 404, "NoSuchBucket").await?;
                continue;
            }
        };

        if method == "HEAD" {
            let barrier = state.lock().expect("lock").head_barrier.clone();
            if let Some(barrier) = barrier {
                let result = barrier.wait().await;
                if result.is_leader() {
                    state.lock().expect("lock").head_barrier = None;
                }
            }
        }

        match method.as_str() {
            "PUT" => {
                let sha = headers
                    .get("x-amz-meta-sha256")
                    .cloned()
                    .unwrap_or_else(|| hex::encode(Sha256::digest(&body)));
                let conflict = {
                    let mut g = state.lock().expect("lock");
                    if g.objects.contains_key(&key)
                        && headers.get("if-none-match").is_some_and(|v| v == "*")
                    {
                        true
                    } else {
                        g.objects.insert(
                            key.clone(),
                            StoredObject {
                                body,
                                sha256: sha,
                                modified: SystemTime::now(),
                            },
                        );
                        false
                    }
                };
                if conflict {
                    write_status(&mut stream, 412, "Precondition Failed", b"").await?;
                    continue;
                }
                write_status(&mut stream, 200, "OK", b"").await?;
            }
            "HEAD" => {
                let found = {
                    let g = state.lock().expect("lock");
                    g.objects.get(&key).map(|obj| {
                        let mut sha = obj.sha256.clone();
                        if fault == Fault::CorruptMetadata {
                            sha = "ff".repeat(32);
                        }
                        (obj.body.len(), sha)
                    })
                };
                match found {
                    None => write_s3_err(&mut stream, 404, "NoSuchKey").await?,
                    Some((len, sha)) => write_head(&mut stream, len, &sha).await?,
                }
            }
            "GET" => {
                let (found, chunk_size, chunk_delay) = {
                    let g = state.lock().expect("lock");
                    let found = g.objects.get(&key).map(|obj| {
                        let mut body = obj.body.clone();
                        if fault == Fault::CorruptBody {
                            body.push(0x01);
                        }
                        (body, obj.sha256.clone())
                    });
                    (found, g.get_chunk_size, g.get_chunk_delay)
                };
                match found {
                    None => write_s3_err(&mut stream, 404, "NoSuchKey").await?,
                    Some((body, sha)) => {
                        write_get(&mut stream, &body, &sha, chunk_size, chunk_delay).await?;
                    }
                }
            }
            "DELETE" => {
                if fault == Fault::DeleteFail {
                    write_s3_err(&mut stream, 500, "InternalError").await?;
                    continue;
                }
                state.lock().expect("lock").objects.remove(&key);
                write_status(&mut stream, 204, "No Content", b"").await?;
            }
            _ => {
                write_status(&mut stream, 405, "Method Not Allowed", b"").await?;
            }
        }
    }
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn split_uri(uri: &str) -> (String, String) {
    match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (uri.to_string(), String::new()),
    }
}

fn query_param(query: &str, name: &str) -> Option<String> {
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=')
            && k == name
        {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &s[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn read_aws_chunked(
    stream: &mut tokio::net::TcpStream,
    mut buf: Vec<u8>,
    decoded: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut out = Vec::with_capacity(decoded);
    loop {
        while !buf.windows(2).any(|w| w == b"\r\n") {
            let mut tmp = [0_u8; 256];
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let header = String::from_utf8_lossy(&buf[..pos]).into_owned();
        buf.drain(..pos + 2);
        let size_str = header.split(';').next().unwrap_or("0");
        let size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        while buf.len() < size + 2 {
            let mut tmp = vec![0_u8; size + 2 - buf.len()];
            stream.read_exact(&mut tmp).await?;
            buf.extend_from_slice(&tmp);
        }
        out.extend_from_slice(&buf[..size]);
        buf.drain(..size + 2);
        if out.len() >= decoded {
            break;
        }
    }
    out.truncate(decoded);
    Ok(out)
}

fn list_xml(state: &Arc<Mutex<Inner>>, bucket: &str, prefix: &str) -> String {
    let g = state.lock().expect("lock");
    let mut contents = String::new();
    for (key, obj) in &g.objects {
        if key.starts_with(prefix) {
            let lm = if g.omit_last_modified {
                String::new()
            } else {
                "<LastModified>2020-01-01T00:00:00.000Z</LastModified>".to_string()
            };
            contents.push_str(&format!(
                "<Contents><Key>{key}</Key>{lm}<ETag>\"etag\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                obj.body.len()
            ));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{bucket}</Name><Prefix>{prefix}</Prefix><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
    )
}

async fn write_status(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    reason: &str,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn write_xml(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    xml: &str,
) -> Result<(), std::io::Error> {
    let resp = format!(
        "HTTP/1.1 {code} OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{xml}",
        xml.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

async fn write_s3_err(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    name: &str,
) -> Result<(), std::io::Error> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{name}</Code><Message>{name}</Message></Error>"
    );
    let reason = match code {
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{xml}",
        xml.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

async fn write_head(
    stream: &mut tokio::net::TcpStream,
    len: usize,
    sha: &str,
) -> Result<(), std::io::Error> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nx-amz-meta-sha256: {sha}\r\nConnection: keep-alive\r\n\r\n"
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

async fn write_get(
    stream: &mut tokio::net::TcpStream,
    body: &[u8],
    sha: &str,
    chunk_size: usize,
    chunk_delay: Duration,
) -> Result<(), std::io::Error> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nx-amz-meta-sha256: {sha}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    if chunk_delay.is_zero() || chunk_size >= body.len() {
        stream.write_all(body).await?;
    } else {
        for chunk in body.chunks(chunk_size) {
            tokio::time::sleep(chunk_delay).await;
            stream.write_all(chunk).await?;
            stream.flush().await?;
        }
    }
    stream.flush().await
}

#[cfg(test)]
mod tests {
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
}
