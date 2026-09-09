use super::*;

pub(super) fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

pub(super) fn split_uri(uri: &str) -> (String, String) {
    match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (uri.to_string(), String::new()),
    }
}

pub(super) fn query_param(query: &str, name: &str) -> Option<String> {
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=')
            && k == name
        {
            return Some(percent_decode(v));
        }
    }
    None
}

pub(super) fn percent_decode(s: &str) -> String {
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

pub(super) async fn read_aws_chunked(
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

pub(super) fn list_xml(
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

pub(super) async fn write_status(
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

pub(super) async fn write_xml(
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

pub(super) async fn write_s3_err(
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

pub(super) async fn write_object_status(
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

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub(super) async fn write_get(
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

pub(super) async fn write_get_prefix_then_reset(
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

pub(super) fn etag_header_matches(header: &str, etag: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_matches('"') == etag)
}

pub(super) fn apply_range(header: &str, body: &[u8]) -> Option<(usize, usize)> {
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

pub(super) fn xml_values(xml: &str, tag: &str) -> Vec<String> {
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

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub(super) fn meta_from_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            name.strip_prefix("x-amz-meta-")
                .map(|name| (name.to_owned(), value.clone()))
        })
        .collect()
}

pub(super) fn http_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
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

pub(super) fn storage_class_from(headers: &HashMap<String, String>) -> String {
    headers
        .get("x-amz-storage-class")
        .cloned()
        .unwrap_or_else(|| "STANDARD".to_owned())
}

pub(super) fn parse_ssec(headers: &HashMap<String, String>) -> Result<Option<String>, ()> {
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

pub(super) fn ssec_denied(headers: &HashMap<String, String>, stored: Option<&str>) -> bool {
    let Some(expected) = stored else {
        return false;
    };
    parse_ssec(headers).ok().flatten().as_deref() != Some(expected)
}
