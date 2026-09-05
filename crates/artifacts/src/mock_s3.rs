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
    PutResponseLoss,
    DeleteResponseLoss,
    CreateResponseLoss,
    CompleteResponseLoss,
    AbortResponseLoss,
    MidstreamReset,
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
    /// Present when the request carried SSE-C headers. Never stores the key.
    pub ssec_algorithm: Option<String>,
    /// Public SSE-C key MD5 header, if any.
    pub ssec_key_md5: Option<String>,
    /// Physical S3 storage class requested by the adapter, if any.
    pub storage_class: Option<String>,
}

#[derive(Clone)]
pub(crate) struct StoredObject {
    pub body: Vec<u8>,
    pub sha256: String,
    pub etag: String,
    pub metadata: HashMap<String, String>,
    pub response_headers: HashMap<String, String>,
    #[allow(dead_code)]
    pub modified: SystemTime,
    pub storage_class: String,
    pub ssec_key_md5: Option<String>,
}

struct MultipartUpload {
    key: String,
    parts: std::collections::BTreeMap<i32, (String, Vec<u8>)>,
    metadata: HashMap<String, String>,
    response_headers: HashMap<String, String>,
    storage_class: String,
    ssec_key_md5: Option<String>,
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
    uploads: HashMap<String, MultipartUpload>,
    fault: Fault,
    recorded: Vec<Recorded>,
    get_chunk_size: usize,
    get_chunk_delay: Duration,
    omit_last_modified: bool,
    head_barrier: Option<Arc<tokio::sync::Barrier>>,
    conditional_put_race: Option<Vec<u8>>,
}

impl MockS3 {
    pub async fn spawn(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(Inner {
            bucket: bucket.to_string(),
            objects: HashMap::new(),
            uploads: HashMap::new(),
            fault: Fault::None,
            recorded: Vec::new(),
            get_chunk_size: usize::MAX,
            get_chunk_delay: Duration::ZERO,
            omit_last_modified: false,
            head_barrier: None,
            conditional_put_race: None,
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

    /// Insert one competing object immediately before the next conditional create fence.
    pub fn race_next_conditional_put(&self, body: Vec<u8>) {
        self.state.lock().expect("lock").conditional_put_race = Some(body);
    }

    /// Number of provider multipart uploads that have not completed or aborted.
    pub fn multipart_upload_count(&self) -> usize {
        self.state.lock().expect("lock").uploads.len()
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
                etag: hex::encode(md5::Md5::digest(&body)),
                body,
                sha256,
                metadata: HashMap::new(),
                response_headers: HashMap::new(),
                modified: SystemTime::now(),
                storage_class: "STANDARD".to_owned(),
                ssec_key_md5: None,
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
                ssec_algorithm: headers
                    .get("x-amz-server-side-encryption-customer-algorithm")
                    .cloned(),
                ssec_key_md5: headers
                    .get("x-amz-server-side-encryption-customer-key-md5")
                    .cloned(),
                storage_class: headers.get("x-amz-storage-class").cloned(),
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
            && query
                .split('&')
                .any(|part| part == "uploads" || part == "uploads=")
        {
            let list_prefix = query_param(&query, "prefix").unwrap_or_default();
            let mut uploads = state
                .lock()
                .expect("lock")
                .uploads
                .iter()
                .filter(|(_, upload)| upload.key.starts_with(&list_prefix))
                .map(|(id, upload)| (upload.key.clone(), id.clone()))
                .collect::<Vec<_>>();
            uploads.sort();
            let entries = uploads
                .into_iter()
                .map(|(key, id)| {
                    format!(
                        "<Upload><Key>{}</Key><UploadId>{}</UploadId></Upload>",
                        xml_escape(&key),
                        xml_escape(&id)
                    )
                })
                .collect::<String>();
            write_xml(
                &mut stream,
                200,
                &format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListMultipartUploadsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Bucket>{}</Bucket><IsTruncated>false</IsTruncated>{entries}</ListMultipartUploadsResult>",
                    xml_escape(&bucket)
                ),
            )
            .await?;
            continue;
        }

        if method == "GET"
            && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
            && (query.contains("list-type=2") || query.contains("prefix="))
        {
            let list_prefix = query_param(&query, "prefix").unwrap_or_default();
            let delimiter = query_param(&query, "delimiter");
            let continuation = query_param(&query, "continuation-token");
            let start_after = query_param(&query, "start-after");
            let max_keys = query_param(&query, "max-keys")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1000)
                .min(1000);
            let xml = list_xml(
                &state,
                &bucket,
                &list_prefix,
                delimiter.as_deref(),
                continuation.as_deref(),
                start_after.as_deref(),
                max_keys,
            );
            write_xml(&mut stream, 200, &xml).await?;
            continue;
        }

        if method == "POST"
            && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
            && (query == "delete" || query.starts_with("delete="))
        {
            if fault == Fault::DeleteFail {
                write_s3_err(&mut stream, 500, "InternalError").await?;
                continue;
            }
            let xml = String::from_utf8_lossy(&body);
            let keys = xml_values(&xml, "Key");
            {
                let mut g = state.lock().expect("lock");
                for key in &keys {
                    g.objects.remove(key);
                }
            }
            if fault == Fault::DeleteResponseLoss {
                stream.shutdown().await?;
                return Ok(());
            }
            let deleted = keys
                .iter()
                .map(|key| format!("<Deleted><Key>{}</Key></Deleted>", xml_escape(key)))
                .collect::<String>();
            write_xml(
                &mut stream,
                200,
                &format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{deleted}</DeleteResult>"
                ),
            )
            .await?;
            continue;
        }

        let key = match path.strip_prefix(&prefix) {
            Some(k) => percent_decode(k),
            None => {
                write_s3_err(&mut stream, 404, "NoSuchBucket").await?;
                continue;
            }
        };

        if method == "POST" && (query == "uploads" || query.starts_with("uploads=")) {
            let Ok(ssec) = parse_ssec(&headers) else {
                write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                continue;
            };
            let upload_id = format!(
                "upload-{}",
                SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos())
            );
            let metadata = meta_from_headers(&headers);
            let response_headers = http_headers(&headers);
            state.lock().expect("lock").uploads.insert(
                upload_id.clone(),
                MultipartUpload {
                    key: key.clone(),
                    parts: std::collections::BTreeMap::new(),
                    metadata,
                    response_headers,
                    storage_class: storage_class_from(&headers),
                    ssec_key_md5: ssec,
                },
            );
            if fault == Fault::CreateResponseLoss {
                stream.shutdown().await?;
                return Ok(());
            }
            write_xml(
                &mut stream,
                200,
                &format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><InitiateMultipartUploadResult><Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId></InitiateMultipartUploadResult>",
                    xml_escape(&bucket),
                    xml_escape(&key),
                    xml_escape(&upload_id)
                ),
            )
            .await?;
            continue;
        }
        if method == "PUT"
            && let (Some(part_number), Some(upload_id)) = (
                query_param(&query, "partNumber").and_then(|value| value.parse::<i32>().ok()),
                query_param(&query, "uploadId"),
            )
        {
            let Ok(ssec) = parse_ssec(&headers) else {
                write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                continue;
            };
            let etag = hex::encode(md5::Md5::digest(&body));
            let ok = {
                let mut g = state.lock().expect("lock");
                match g.uploads.get_mut(&upload_id) {
                    Some(upload) if upload.key == key && upload.ssec_key_md5 == ssec => {
                        upload.parts.insert(part_number, (etag.clone(), body));
                        true
                    }
                    Some(_) | None => false,
                }
            };
            if !ok {
                write_s3_err(&mut stream, 404, "NoSuchUpload").await?;
                continue;
            }
            write_object_status(&mut stream, 200, "OK", 0, &HashMap::new(), &etag, None).await?;
            continue;
        }
        if method == "POST"
            && let Some(upload_id) = query_param(&query, "uploadId")
        {
            let Ok(ssec) = parse_ssec(&headers) else {
                write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                continue;
            };
            let completed = {
                let mut g = state.lock().expect("lock");
                g.uploads.remove(&upload_id)
            };
            let Some(upload) = completed else {
                write_s3_err(&mut stream, 404, "NoSuchUpload").await?;
                continue;
            };
            if upload.key != key {
                write_s3_err(&mut stream, 404, "NoSuchUpload").await?;
                continue;
            }
            if upload.ssec_key_md5 != ssec {
                write_s3_err(&mut stream, 403, "AccessDenied").await?;
                continue;
            }
            let mut assembled = Vec::new();
            let mut part_digests = Vec::new();
            let part_count = upload.parts.len();
            for (_number, (etag, part)) in upload.parts {
                part_digests.extend_from_slice(&hex::decode(etag).expect("stored part MD5"));
                assembled.extend_from_slice(&part);
            }
            let sha256 = hex::encode(Sha256::digest(&assembled));
            let metadata = upload.metadata;
            let etag = format!(
                "{}-{part_count}",
                hex::encode(md5::Md5::digest(part_digests))
            );
            if fault == Fault::CompleteResponseLoss {
                state.lock().expect("lock").objects.insert(
                    key.clone(),
                    StoredObject {
                        sha256,
                        body: assembled,
                        etag: etag.clone(),
                        metadata: metadata.clone(),
                        response_headers: upload.response_headers.clone(),
                        modified: SystemTime::now(),
                        storage_class: upload.storage_class.clone(),
                        ssec_key_md5: upload.ssec_key_md5.clone(),
                    },
                );
                stream.shutdown().await?;
                return Ok(());
            }
            state.lock().expect("lock").objects.insert(
                key.clone(),
                StoredObject {
                    sha256,
                    body: assembled,
                    etag: etag.clone(),
                    metadata,
                    response_headers: upload.response_headers,
                    modified: SystemTime::now(),
                    storage_class: upload.storage_class,
                    ssec_key_md5: upload.ssec_key_md5,
                },
            );
            write_xml(
                &mut stream,
                200,
                &format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUploadResult><Key>{}</Key><ETag>\"{}\"</ETag></CompleteMultipartUploadResult>",
                    xml_escape(&key),
                    xml_escape(&etag)
                ),
            )
            .await?;
            continue;
        }
        if method == "DELETE"
            && let Some(upload_id) = query_param(&query, "uploadId")
        {
            state.lock().expect("lock").uploads.remove(&upload_id);
            if fault == Fault::AbortResponseLoss {
                stream.shutdown().await?;
                return Ok(());
            }
            write_status(&mut stream, 204, "No Content", b"").await?;
            continue;
        }

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
                let metadata = headers
                    .iter()
                    .filter_map(|(name, value)| {
                        name.strip_prefix("x-amz-meta-")
                            .map(|name| (name.to_owned(), value.clone()))
                    })
                    .collect::<HashMap<_, _>>();
                let response_headers = headers
                    .iter()
                    .filter(|(name, _)| {
                        matches!(
                            name.as_str(),
                            "content-type"
                                | "content-language"
                                | "content-disposition"
                                | "content-encoding"
                                | "cache-control"
                                | "expires"
                                | "x-amz-storage-class"
                        )
                    })
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<HashMap<_, _>>();
                let etag = hex::encode(md5::Md5::digest(&body));
                let Ok(ssec) = parse_ssec(&headers) else {
                    write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                    continue;
                };
                let storage_class = storage_class_from(&headers);
                let conflict = {
                    let mut g = state.lock().expect("lock");
                    if headers
                        .get("if-none-match")
                        .is_some_and(|value| value == "*")
                        && let Some(raced_body) = g.conditional_put_race.take()
                    {
                        let raced_checksums = crate::hash_bytes(&raced_body);
                        let mut raced_metadata = metadata.clone();
                        raced_metadata
                            .insert("oc-r2-md5".to_owned(), hex::encode(raced_checksums.md5));
                        raced_metadata
                            .insert("oc-r2-sha1".to_owned(), hex::encode(raced_checksums.sha1));
                        raced_metadata.insert(
                            "oc-r2-sha256".to_owned(),
                            hex::encode(raced_checksums.sha256),
                        );
                        raced_metadata.insert(
                            "oc-r2-sha384".to_owned(),
                            hex::encode(raced_checksums.sha384),
                        );
                        raced_metadata.insert(
                            "oc-r2-sha512".to_owned(),
                            hex::encode(raced_checksums.sha512),
                        );
                        let raced_etag = hex::encode(md5::Md5::digest(&raced_body));
                        g.objects.insert(
                            key.clone(),
                            StoredObject {
                                sha256: hex::encode(raced_checksums.sha256),
                                body: raced_body,
                                etag: raced_etag,
                                metadata: raced_metadata,
                                response_headers: response_headers.clone(),
                                modified: SystemTime::now(),
                                storage_class: storage_class.clone(),
                                ssec_key_md5: ssec.clone(),
                            },
                        );
                    }
                    let current = g.objects.get(&key);
                    let none_failed = headers.get("if-none-match").is_some_and(|value| {
                        value == "*" && current.is_some()
                            || current
                                .is_some_and(|object| etag_header_matches(value, &object.etag))
                    });
                    let match_failed = headers.get("if-match").is_some_and(|value| {
                        current.is_none_or(|object| !etag_header_matches(value, &object.etag))
                    });
                    if none_failed || match_failed {
                        true
                    } else {
                        g.objects.insert(
                            key.clone(),
                            StoredObject {
                                body,
                                sha256: sha,
                                etag: etag.clone(),
                                metadata,
                                response_headers,
                                modified: SystemTime::now(),
                                storage_class,
                                ssec_key_md5: ssec,
                            },
                        );
                        false
                    }
                };
                if conflict {
                    write_status(&mut stream, 412, "Precondition Failed", b"").await?;
                    continue;
                }
                if fault == Fault::PutResponseLoss {
                    stream.shutdown().await?;
                    return Ok(());
                }
                write_object_status(&mut stream, 200, "OK", 0, &HashMap::new(), &etag, None)
                    .await?;
            }
            "HEAD" => {
                let found = {
                    let g = state.lock().expect("lock");
                    g.objects.get(&key).map(|obj| {
                        let mut metadata = obj.metadata.clone();
                        metadata
                            .entry("sha256".to_owned())
                            .or_insert_with(|| obj.sha256.clone());
                        if fault == Fault::CorruptMetadata {
                            if metadata.contains_key("oc-r2-schema") {
                                metadata.insert("oc-r2-schema".to_owned(), "corrupt".to_owned());
                            } else {
                                metadata.insert("sha256".to_owned(), "ff".repeat(32));
                            }
                        }
                        (
                            obj.body.len(),
                            metadata,
                            obj.response_headers.clone(),
                            obj.etag.clone(),
                            obj.ssec_key_md5.clone(),
                        )
                    })
                };
                match found {
                    None => write_s3_err(&mut stream, 404, "NoSuchKey").await?,
                    Some((len, metadata, response_headers, etag, ssec_md5)) => {
                        if ssec_denied(&headers, ssec_md5.as_deref()) {
                            write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                            continue;
                        }
                        if headers
                            .get("if-match")
                            .is_some_and(|value| !etag_header_matches(value, &etag))
                        {
                            write_status(&mut stream, 412, "Precondition Failed", b"").await?;
                            continue;
                        }
                        write_object_status(
                            &mut stream,
                            200,
                            "OK",
                            len,
                            &metadata,
                            &etag,
                            Some(&response_headers),
                        )
                        .await?;
                    }
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
                        let mut metadata = obj.metadata.clone();
                        metadata
                            .entry("sha256".to_owned())
                            .or_insert_with(|| obj.sha256.clone());
                        (
                            body,
                            metadata,
                            obj.response_headers.clone(),
                            obj.etag.clone(),
                            obj.ssec_key_md5.clone(),
                        )
                    });
                    (found, g.get_chunk_size, g.get_chunk_delay)
                };
                match found {
                    None => write_s3_err(&mut stream, 404, "NoSuchKey").await?,
                    Some((body, metadata, response_headers, etag, ssec_md5)) => {
                        if ssec_denied(&headers, ssec_md5.as_deref()) {
                            write_s3_err(&mut stream, 400, "InvalidRequest").await?;
                            continue;
                        }
                        if headers
                            .get("if-match")
                            .is_some_and(|value| !etag_header_matches(value, &etag))
                            || headers
                                .get("if-none-match")
                                .is_some_and(|value| etag_header_matches(value, &etag))
                        {
                            write_status(&mut stream, 412, "Precondition Failed", b"").await?;
                            continue;
                        }
                        let full_length = body.len();
                        let range = match headers.get("range") {
                            Some(value) => match apply_range(value, &body) {
                                Some(value) => Some(value),
                                None => {
                                    write_status(&mut stream, 416, "Range Not Satisfiable", b"")
                                        .await?;
                                    continue;
                                }
                            },
                            None => None,
                        };
                        let (status, content_range, returned) = match range {
                            Some((start, end)) => (
                                206,
                                Some(format!("bytes {start}-{end}/{full_length}")),
                                body[start..=end].to_vec(),
                            ),
                            None => (200, None, body),
                        };
                        if fault == Fault::MidstreamReset {
                            write_get_prefix_then_reset(
                                &mut stream,
                                status,
                                &returned,
                                &metadata,
                                &response_headers,
                                &etag,
                                content_range.as_deref(),
                            )
                            .await?;
                            return Ok(());
                        } else {
                            write_get(
                                &mut stream,
                                status,
                                &returned,
                                &metadata,
                                &response_headers,
                                &etag,
                                content_range.as_deref(),
                                chunk_size,
                                chunk_delay,
                            )
                            .await?;
                        }
                    }
                }
            }
            "DELETE" => {
                if fault == Fault::DeleteFail {
                    write_s3_err(&mut stream, 500, "InternalError").await?;
                    continue;
                }
                state.lock().expect("lock").objects.remove(&key);
                if fault == Fault::DeleteResponseLoss {
                    stream.shutdown().await?;
                    return Ok(());
                }
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

fn list_xml(
    state: &Arc<Mutex<Inner>>,
    bucket: &str,
    prefix: &str,
    delimiter: Option<&str>,
    continuation: Option<&str>,
    start_after: Option<&str>,
    max_keys: usize,
) -> String {
    let g = state.lock().expect("lock");
    let after = continuation.or(start_after);
    let mut rows = g
        .objects
        .iter()
        .filter(|(key, _)| key.starts_with(prefix))
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut contents = String::new();
    let mut common = std::collections::BTreeSet::new();
    let mut emitted = 0_usize;
    let mut truncated = false;
    let mut last_emitted = None;
    for (key, obj) in rows {
        if emitted >= max_keys {
            truncated = true;
            break;
        }
        if let Some(delimiter) = delimiter {
            let remainder = &key[prefix.len()..];
            if let Some(position) = remainder.find(delimiter) {
                let end = prefix.len() + position + delimiter.len();
                common.insert(key[..end].to_owned());
                emitted = emitted.saturating_add(1);
                last_emitted = Some(key.clone());
                continue;
            }
        }
        let lm = if g.omit_last_modified {
            String::new()
        } else {
            "<LastModified>2020-01-01T00:00:00.000Z</LastModified>".to_string()
        };
        contents.push_str(&format!(
            "<Contents><Key>{}</Key>{lm}<ETag>\"{}\"</ETag><Size>{}</Size><StorageClass>{}</StorageClass></Contents>",
            xml_escape(key),
            obj.etag,
            obj.body.len(),
            xml_escape(&obj.storage_class)
        ));
        emitted = emitted.saturating_add(1);
        last_emitted = Some(key.clone());
    }
    let common = common
        .iter()
        .map(|prefix| {
            format!(
                "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                xml_escape(prefix)
            )
        })
        .collect::<String>();
    let token = truncated
        .then_some(last_emitted)
        .flatten()
        .map_or_else(String::new, |token| {
            format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                xml_escape(&token)
            )
        });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{bucket}</Name><Prefix>{}</Prefix><MaxKeys>{max_keys}</MaxKeys><KeyCount>{emitted}</KeyCount><IsTruncated>{truncated}</IsTruncated>{token}{contents}{common}</ListBucketResult>",
        xml_escape(prefix)
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

async fn write_object_status(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    reason: &str,
    len: usize,
    metadata: &HashMap<String, String>,
    etag: &str,
    response_headers: Option<&HashMap<String, String>>,
) -> Result<(), std::io::Error> {
    let mut extra = format!("ETag: \"{etag}\"\r\nLast-Modified: Wed, 01 Jan 2020 00:00:00 GMT\r\n");
    let mut metadata = metadata.iter().collect::<Vec<_>>();
    metadata.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in metadata {
        extra.push_str(&format!("x-amz-meta-{name}: {value}\r\n"));
    }
    if let Some(headers) = response_headers {
        let mut headers = headers.iter().collect::<Vec<_>>();
        headers.sort_by(|left, right| left.0.cmp(right.0));
        for (name, value) in headers {
            extra.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {len}\r\n{extra}Connection: keep-alive\r\n\r\n"
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

#[allow(clippy::too_many_arguments)]
async fn write_get(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    body: &[u8],
    metadata: &HashMap<String, String>,
    response_headers: &HashMap<String, String>,
    etag: &str,
    content_range: Option<&str>,
    chunk_size: usize,
    chunk_delay: Duration,
) -> Result<(), std::io::Error> {
    let reason = if code == 206 { "Partial Content" } else { "OK" };
    let mut extra = format!("ETag: \"{etag}\"\r\nLast-Modified: Wed, 01 Jan 2020 00:00:00 GMT\r\n");
    if let Some(content_range) = content_range {
        extra.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    let mut metadata = metadata.iter().collect::<Vec<_>>();
    metadata.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in metadata {
        extra.push_str(&format!("x-amz-meta-{name}: {value}\r\n"));
    }
    let mut headers = response_headers.iter().collect::<Vec<_>>();
    headers.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in headers {
        extra.push_str(&format!("{name}: {value}\r\n"));
    }
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\n{extra}Connection: keep-alive\r\n\r\n",
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

async fn write_get_prefix_then_reset(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    body: &[u8],
    metadata: &HashMap<String, String>,
    response_headers: &HashMap<String, String>,
    etag: &str,
    content_range: Option<&str>,
) -> Result<(), std::io::Error> {
    let reason = if code == 206 { "Partial Content" } else { "OK" };
    let mut extra = format!("ETag: \"{etag}\"\r\nLast-Modified: Wed, 01 Jan 2020 00:00:00 GMT\r\n");
    if let Some(content_range) = content_range {
        extra.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    let mut metadata = metadata.iter().collect::<Vec<_>>();
    metadata.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in metadata {
        extra.push_str(&format!("x-amz-meta-{name}: {value}\r\n"));
    }
    let mut headers = response_headers.iter().collect::<Vec<_>>();
    headers.sort_by(|left, right| left.0.cmp(right.0));
    for (name, value) in headers {
        extra.push_str(&format!("{name}: {value}\r\n"));
    }
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&body[..body.len() / 2]).await?;
    stream.shutdown().await
}

fn etag_header_matches(header: &str, etag: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_matches('"') == etag)
}

fn apply_range(header: &str, body: &[u8]) -> Option<(usize, usize)> {
    let value = header.strip_prefix("bytes=")?;
    if value.contains(',') || body.is_empty() {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<usize>().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = body.len().saturating_sub(suffix);
        return Some((start, body.len() - 1));
    }
    let start = start.parse::<usize>().ok()?;
    if start >= body.len() {
        return None;
    }
    let end = if end.is_empty() {
        body.len() - 1
    } else {
        end.parse::<usize>().ok()?.min(body.len() - 1)
    };
    (start <= end).then_some((start, end))
}

fn xml_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut remaining = xml;
    while let Some(start) = remaining.find(&open) {
        let after = &remaining[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        values.push(xml_unescape(&after[..end]));
        remaining = &after[end + close.len()..];
    }
    values
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn meta_from_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            name.strip_prefix("x-amz-meta-")
                .map(|name| (name.to_owned(), value.clone()))
        })
        .collect()
}

fn http_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type"
                    | "content-language"
                    | "content-disposition"
                    | "content-encoding"
                    | "cache-control"
                    | "expires"
                    | "x-amz-storage-class"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn storage_class_from(headers: &HashMap<String, String>) -> String {
    headers
        .get("x-amz-storage-class")
        .cloned()
        .unwrap_or_else(|| "STANDARD".to_owned())
}

fn parse_ssec(headers: &HashMap<String, String>) -> Result<Option<String>, ()> {
    let algo = headers.get("x-amz-server-side-encryption-customer-algorithm");
    let key = headers.get("x-amz-server-side-encryption-customer-key");
    let md5 = headers.get("x-amz-server-side-encryption-customer-key-md5");
    match (algo, key, md5) {
        (None, None, None) => Ok(None),
        (Some(algo), Some(key), Some(md5)) if algo.eq_ignore_ascii_case("AES256") => {
            let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, key)
                .map_err(|_| ())?;
            if raw.len() != 32 {
                return Err(());
            }
            let computed = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                md5::Md5::digest(&raw),
            );
            if &computed != md5 {
                return Err(());
            }
            Ok(Some(computed))
        }
        _ => Err(()),
    }
}

fn ssec_denied(headers: &HashMap<String, String>, stored: Option<&str>) -> bool {
    let Some(expected) = stored else {
        return false;
    };
    parse_ssec(headers).ok().flatten().as_deref() != Some(expected)
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
