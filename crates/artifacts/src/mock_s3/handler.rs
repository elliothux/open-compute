use super::*;

struct ParsedRequest {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    fault: Fault,
}

enum SpecialRoute {
    Handled,
    Close,
    Object(String),
}

pub(super) async fn handle_conn(
    mut stream: tokio::net::TcpStream,
    state: Arc<Mutex<Inner>>,
) -> Result<(), std::io::Error> {
    loop {
        let Some(mut request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        record_request(&state, &request);
        request.fault = state.lock().expect("lock").fault;
        if apply_fault(&mut stream, &request).await? {
            continue;
        }
        let key = match handle_special_routes(&mut stream, &state, &mut request).await? {
            SpecialRoute::Handled => continue,
            SpecialRoute::Close => return Ok(()),
            SpecialRoute::Object(key) => key,
        };
        handle_object_route(&mut stream, &state, request, key).await?;
    }
}

async fn read_request(
    stream: &mut tokio::net::TcpStream,
) -> Result<Option<ParsedRequest>, std::io::Error> {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 1024];
    let header_end;
    loop {
        let n = match stream.read(&mut tmp).await {
            Ok(0) => return Ok(None),
            Ok(n) => n,
            Err(error) if error.kind() == ErrorKind::ConnectionReset => return Ok(None),
            Err(error) => return Err(error),
        };
        buf.extend_from_slice(&tmp[..n]);
        if let Some(position) = find_headers_end(&buf) {
            header_end = position;
            break;
        }
        if buf.len() > 1024 * 1024 {
            write_status(stream, 400, "Bad Request", b"").await?;
            return Ok(None);
        }
    }
    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_text.split("\r\n");
    let mut request_line = lines.next().unwrap_or("").split_whitespace();
    let method = request_line.next().unwrap_or("").to_owned();
    let (path, query) = split_uri(request_line.next().unwrap_or(""));
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let decoded = headers
        .get("x-amz-decoded-content-length")
        .and_then(|value| value.parse::<usize>().ok());
    let mut body = buf[header_end..].to_vec();
    let extra_len = body.len();
    if let Some(decoded) = decoded {
        body = read_aws_chunked(stream, body, decoded).await?;
    } else if body.len() < content_length {
        body.resize(content_length, 0);
        stream
            .read_exact(&mut body[extra_len.min(content_length)..])
            .await?;
        body.truncate(content_length);
    } else {
        body.truncate(content_length);
    }
    Ok(Some(ParsedRequest {
        method,
        path,
        query,
        headers,
        body,
        fault: Fault::None,
    }))
}

fn record_request(state: &Arc<Mutex<Inner>>, request: &ParsedRequest) {
    let authorization = request.headers.get("authorization").cloned();
    state.lock().expect("lock").recorded.push(Recorded {
        method: request.method.clone(),
        path: request.path.clone(),
        query: request.query.clone(),
        has_authorization: authorization.is_some(),
        authorization,
        ssec_algorithm: request
            .headers
            .get("x-amz-server-side-encryption-customer-algorithm")
            .cloned(),
        ssec_key_md5: request
            .headers
            .get("x-amz-server-side-encryption-customer-key-md5")
            .cloned(),
        storage_class: request.headers.get("x-amz-storage-class").cloned(),
    });
}

async fn apply_fault(
    stream: &mut tokio::net::TcpStream,
    request: &ParsedRequest,
) -> Result<bool, std::io::Error> {
    if request.method == "DELETE" && request.fault != Fault::DeleteFail {
        return Ok(false);
    }
    let response = match request.fault {
        Fault::Auth => Some((403, "InvalidAccessKeyId")),
        Fault::Permission => Some((403, "AccessDenied")),
        Fault::ServerError => Some((500, "InternalError")),
        Fault::NotFound => Some((404, "NoSuchKey")),
        Fault::Timeout => {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Some((500, "InternalError"))
        }
        _ => None,
    };
    if let Some((status, code)) = response {
        write_s3_err(stream, status, code).await?;
        return Ok(true);
    }
    Ok(false)
}

async fn handle_special_routes(
    stream: &mut tokio::net::TcpStream,
    state: &Arc<Mutex<Inner>>,
    request: &mut ParsedRequest,
) -> Result<SpecialRoute, std::io::Error> {
    let method = request.method.as_str();
    let path = request.path.as_str();
    let query = request.query.as_str();
    let headers = &request.headers;
    let body = &mut request.body;
    let fault = request.fault;
    let bucket = state.lock().expect("lock").bucket.clone();
    let prefix = format!("/{bucket}/");
    if method == "HEAD" && (path == format!("/{bucket}") || path == prefix) {
        write_status(stream, 200, "OK", b"").await?;
        return Ok(SpecialRoute::Handled);
    }

    if method == "GET"
        && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
        && query
            .split('&')
            .any(|part| part == "uploads" || part == "uploads=")
    {
        let list_prefix = query_param(query, "prefix").unwrap_or_default();
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
            stream,
            200,
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListMultipartUploadsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Bucket>{}</Bucket><IsTruncated>false</IsTruncated>{entries}</ListMultipartUploadsResult>",
                xml_escape(&bucket)
            ),
        )
        .await?;
        return Ok(SpecialRoute::Handled);
    }

    if method == "GET"
        && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
        && (query.contains("list-type=2") || query.contains("prefix="))
    {
        let list_prefix = query_param(query, "prefix").unwrap_or_default();
        let delimiter = query_param(query, "delimiter");
        let continuation = query_param(query, "continuation-token");
        let start_after = query_param(query, "start-after");
        let max_keys = query_param(query, "max-keys")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1000)
            .min(1000);
        let xml = list_xml(
            state,
            &bucket,
            &list_prefix,
            delimiter.as_deref(),
            continuation.as_deref(),
            start_after.as_deref(),
            max_keys,
        );
        write_xml(stream, 200, &xml).await?;
        return Ok(SpecialRoute::Handled);
    }

    if method == "POST"
        && (path == format!("/{bucket}") || path == format!("/{bucket}/"))
        && (query == "delete" || query.starts_with("delete="))
    {
        if fault == Fault::DeleteFail {
            write_s3_err(stream, 500, "InternalError").await?;
            return Ok(SpecialRoute::Handled);
        }
        let xml = String::from_utf8_lossy(body);
        let keys = xml_values(&xml, "Key");
        {
            let mut g = state.lock().expect("lock");
            for key in &keys {
                g.objects.remove(key);
            }
        }
        if fault == Fault::DeleteResponseLoss {
            stream.shutdown().await?;
            return Ok(SpecialRoute::Close);
        }
        let deleted = keys
            .iter()
            .map(|key| format!("<Deleted><Key>{}</Key></Deleted>", xml_escape(key)))
            .collect::<String>();
        write_xml(
            stream,
            200,
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{deleted}</DeleteResult>"
            ),
        )
        .await?;
        return Ok(SpecialRoute::Handled);
    }

    let key = match path.strip_prefix(&prefix) {
        Some(k) => percent_decode(k),
        None => {
            write_s3_err(stream, 404, "NoSuchBucket").await?;
            return Ok(SpecialRoute::Handled);
        }
    };

    if method == "POST" && (query == "uploads" || query.starts_with("uploads=")) {
        let Ok(ssec) = parse_ssec(headers) else {
            write_s3_err(stream, 400, "InvalidRequest").await?;
            return Ok(SpecialRoute::Handled);
        };
        let upload_id = format!(
            "upload-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        );
        let metadata = meta_from_headers(headers);
        let response_headers = http_headers(headers);
        state.lock().expect("lock").uploads.insert(
            upload_id.clone(),
            MultipartUpload {
                key: key.clone(),
                parts: std::collections::BTreeMap::new(),
                metadata,
                response_headers,
                storage_class: storage_class_from(headers),
                ssec_key_md5: ssec,
            },
        );
        if fault == Fault::CreateResponseLoss {
            stream.shutdown().await?;
            return Ok(SpecialRoute::Close);
        }
        write_xml(
            stream,
            200,
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><InitiateMultipartUploadResult><Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId></InitiateMultipartUploadResult>",
                xml_escape(&bucket),
                xml_escape(&key),
                xml_escape(&upload_id)
            ),
        )
        .await?;
        return Ok(SpecialRoute::Handled);
    }
    if method == "PUT"
        && let (Some(part_number), Some(upload_id)) = (
            query_param(query, "partNumber").and_then(|value| value.parse::<i32>().ok()),
            query_param(query, "uploadId"),
        )
    {
        let Ok(ssec) = parse_ssec(headers) else {
            write_s3_err(stream, 400, "InvalidRequest").await?;
            return Ok(SpecialRoute::Handled);
        };
        let etag = hex::encode(md5::Md5::digest(body.as_slice()));
        let ok = {
            let mut g = state.lock().expect("lock");
            match g.uploads.get_mut(&upload_id) {
                Some(upload) if upload.key == key && upload.ssec_key_md5 == ssec => {
                    upload
                        .parts
                        .insert(part_number, (etag.clone(), std::mem::take(body)));
                    true
                }
                Some(_) | None => false,
            }
        };
        if !ok {
            write_s3_err(stream, 404, "NoSuchUpload").await?;
            return Ok(SpecialRoute::Handled);
        }
        write_object_status(stream, 200, "OK", 0, &HashMap::new(), &etag, None).await?;
        return Ok(SpecialRoute::Handled);
    }
    if method == "POST"
        && let Some(upload_id) = query_param(query, "uploadId")
    {
        let Ok(ssec) = parse_ssec(headers) else {
            write_s3_err(stream, 400, "InvalidRequest").await?;
            return Ok(SpecialRoute::Handled);
        };
        let completed = {
            let mut g = state.lock().expect("lock");
            g.uploads.remove(&upload_id)
        };
        let Some(upload) = completed else {
            write_s3_err(stream, 404, "NoSuchUpload").await?;
            return Ok(SpecialRoute::Handled);
        };
        if upload.key != key {
            write_s3_err(stream, 404, "NoSuchUpload").await?;
            return Ok(SpecialRoute::Handled);
        }
        if upload.ssec_key_md5 != ssec {
            write_s3_err(stream, 403, "AccessDenied").await?;
            return Ok(SpecialRoute::Handled);
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
            return Ok(SpecialRoute::Close);
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
            stream,
            200,
            &format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUploadResult><Key>{}</Key><ETag>\"{}\"</ETag></CompleteMultipartUploadResult>",
                xml_escape(&key),
                xml_escape(&etag)
            ),
        )
        .await?;
        return Ok(SpecialRoute::Handled);
    }
    if method == "DELETE"
        && let Some(upload_id) = query_param(query, "uploadId")
    {
        state.lock().expect("lock").uploads.remove(&upload_id);
        if fault == Fault::AbortResponseLoss {
            stream.shutdown().await?;
            return Ok(SpecialRoute::Close);
        }
        write_status(stream, 204, "No Content", b"").await?;
        return Ok(SpecialRoute::Handled);
    }
    Ok(SpecialRoute::Object(key))
}

async fn handle_object_route(
    stream: &mut tokio::net::TcpStream,
    state: &Arc<Mutex<Inner>>,
    request: ParsedRequest,
    key: String,
) -> Result<(), std::io::Error> {
    let ParsedRequest {
        method,
        headers,
        body,
        fault,
        ..
    } = request;
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
                write_s3_err(stream, 400, "InvalidRequest").await?;
                return Ok(());
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
                    raced_metadata.insert("oc-r2-md5".to_owned(), hex::encode(raced_checksums.md5));
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
                        || current.is_some_and(|object| etag_header_matches(value, &object.etag))
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
                write_status(stream, 412, "Precondition Failed", b"").await?;
                return Ok(());
            }
            if fault == Fault::PutResponseLoss {
                stream.shutdown().await?;
                return Ok(());
            }
            write_object_status(stream, 200, "OK", 0, &HashMap::new(), &etag, None).await?;
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
                None => write_s3_err(stream, 404, "NoSuchKey").await?,
                Some((len, metadata, response_headers, etag, ssec_md5)) => {
                    if ssec_denied(&headers, ssec_md5.as_deref()) {
                        write_s3_err(stream, 400, "InvalidRequest").await?;
                        return Ok(());
                    }
                    if headers
                        .get("if-match")
                        .is_some_and(|value| !etag_header_matches(value, &etag))
                    {
                        write_status(stream, 412, "Precondition Failed", b"").await?;
                        return Ok(());
                    }
                    write_object_status(
                        stream,
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
                None => write_s3_err(stream, 404, "NoSuchKey").await?,
                Some((body, metadata, response_headers, etag, ssec_md5)) => {
                    if ssec_denied(&headers, ssec_md5.as_deref()) {
                        write_s3_err(stream, 400, "InvalidRequest").await?;
                        return Ok(());
                    }
                    if headers
                        .get("if-match")
                        .is_some_and(|value| !etag_header_matches(value, &etag))
                        || headers
                            .get("if-none-match")
                            .is_some_and(|value| etag_header_matches(value, &etag))
                    {
                        write_status(stream, 412, "Precondition Failed", b"").await?;
                        return Ok(());
                    }
                    let full_length = body.len();
                    let range = match headers.get("range") {
                        Some(value) => match apply_range(value, &body) {
                            Some(value) => Some(value),
                            None => {
                                write_status(stream, 416, "Range Not Satisfiable", b"").await?;
                                return Ok(());
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
                            stream,
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
                            stream,
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
                write_s3_err(stream, 500, "InternalError").await?;
                return Ok(());
            }
            state.lock().expect("lock").objects.remove(&key);
            if fault == Fault::DeleteResponseLoss {
                stream.shutdown().await?;
                return Ok(());
            }
            write_status(stream, 204, "No Content", b"").await?;
        }
        _ => {
            write_status(stream, 405, "Method Not Allowed", b"").await?;
        }
    }
    Ok(())
}
