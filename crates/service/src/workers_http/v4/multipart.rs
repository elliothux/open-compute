//! Bounded streaming parser for Cloudflare Worker multipart uploads.

use super::model::WorkerUploadMetadata;
use axum::body::Body;
use axum::extract::{Multipart, Request};
use axum::http::{HeaderValue, header};
use bytes::Bytes;
use futures::{StreamExt as _, stream};
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, ModuleInput, ModuleType, supports_worker_compatibility,
};
use std::collections::BTreeSet;

const METADATA_PART: &str = "metadata";
const MAX_METADATA_BYTES: usize = 1024 * 1024;
const MULTIPART_OVERHEAD_BYTES: usize = 1024 * 1024;
const MAX_BOUNDARY_BYTES: usize = 70;

#[derive(Clone, Debug)]
struct RawPart {
    name: String,
    file_name: Option<String>,
    content_type: Option<String>,
    bytes: Vec<u8>,
}

/// Fully validated upload ready for the immutable Version pipeline.
#[derive(Clone, Debug)]
pub(crate) struct ParsedWorkerUpload {
    /// Closed metadata emitted by the pinned Wrangler multipart generator.
    pub metadata: WorkerUploadMetadata,
    /// Canonical Worker bundle bytes, absent only for an assets-only Version.
    pub bundle: Option<Vec<u8>>,
}

/// Recover the multipart boundary omitted from the media type by the fixed
/// `cloudflare@7.1.0` `workers.scripts.update()` implementation. That SDK
/// converts the typed body to `FormData` but retains its generated
/// `Content-Type: application/javascript` header. Only a valid RFC multipart
/// opening delimiter is accepted; all other JavaScript bodies still fail
/// closed at the multipart extractor.
pub(crate) async fn normalize_sdk_multipart_request(
    request: Request,
) -> Result<Request, PlatformError> {
    let is_sdk_upload = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("application/javascript"));
    if !is_sdk_upload {
        return Ok(request);
    }

    let (mut parts, body) = request.into_parts();
    let mut body = body.into_data_stream();
    let mut preserved = Vec::<Result<Bytes, axum::Error>>::new();
    let mut opening = Vec::with_capacity(MAX_BOUNDARY_BYTES + 2);
    'opening: loop {
        let chunk = body
            .next()
            .await
            .ok_or_else(invalid)?
            .map_err(|_| invalid())?;
        for byte in chunk.iter().copied() {
            if opening.last() == Some(&b'\r') && byte == b'\n' {
                opening.pop();
                preserved.push(Ok(chunk));
                break 'opening;
            }
            opening.push(byte);
            if opening.len() > MAX_BOUNDARY_BYTES + 2 {
                return Err(invalid());
            }
        }
        preserved.push(Ok(chunk));
    }
    let boundary = opening.strip_prefix(b"--").ok_or_else(invalid)?;
    if boundary.is_empty()
        || boundary.len() > MAX_BOUNDARY_BYTES
        || !boundary.iter().copied().all(valid_boundary_byte)
    {
        return Err(invalid());
    }
    let boundary = std::str::from_utf8(boundary).map_err(|_| invalid())?;
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
            .map_err(|_| invalid())?,
    );
    let prefix = stream::iter(preserved);
    Ok(Request::from_parts(
        parts,
        Body::from_stream(prefix.chain(body)),
    ))
}

fn valid_boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'\'' | b'(' | b')' | b'+' | b'_' | b',' | b'-' | b'.' | b'/' | b':' | b'=' | b'?'
        )
}

/// Incrementally read and bound a Worker multipart request.
pub(crate) async fn parse_worker_upload(
    mut multipart: Multipart,
    limits: BundleLimits,
) -> Result<ParsedWorkerUpload, PlatformError> {
    let total_limit = limits
        .max_total_module_bytes
        .checked_add(MAX_METADATA_BYTES)
        .and_then(|value| value.checked_add(MULTIPART_OVERHEAD_BYTES))
        .ok_or_else(too_large)?;
    let part_limit = limits.max_module_bytes.max(MAX_METADATA_BYTES);
    let mut total = 0_usize;
    let mut names = BTreeSet::new();
    let mut parts = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(|_| invalid())? {
        if parts.len() >= limits.max_modules.saturating_add(1) {
            return Err(too_large());
        }
        let name = field.name().ok_or_else(invalid)?.to_owned();
        validate_part_name(&name)?;
        let file_name = field.file_name().map(ToOwned::to_owned);
        let content_type = field.content_type().map(ToOwned::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|_| invalid())? {
            total = total.checked_add(chunk.len()).ok_or_else(too_large)?;
            if total > total_limit
                || bytes
                    .len()
                    .checked_add(chunk.len())
                    .is_none_or(|value| value > part_limit)
            {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        parts.push(RawPart {
            name,
            file_name,
            content_type,
            bytes,
        });
    }
    normalize_sdk_parts(&mut parts)?;
    for part in &parts {
        if !names.insert(part.name.clone()) {
            return Err(invalid());
        }
    }
    parse_parts(parts, limits)
}

fn normalize_sdk_parts(parts: &mut Vec<RawPart>) -> Result<(), PlatformError> {
    for part in parts.iter_mut().filter(|part| part.name == "files[]") {
        let file_name = part.file_name.take().ok_or_else(invalid)?;
        validate_part_name(&file_name)?;
        part.name = file_name;
    }
    if parts.iter().any(|part| part.name == METADATA_PART) {
        if parts.iter().any(|part| part.name.starts_with("metadata[")) {
            return Err(invalid());
        }
        return Ok(());
    }

    let mut metadata = serde_json::Map::new();
    let mut retained = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        let key = match part.name.as_str() {
            "metadata[main_module]" => Some("main_module"),
            "metadata[body_part]" => Some("body_part"),
            "metadata[compatibility_date]" => Some("compatibility_date"),
            _ if part.name.starts_with("metadata[") => return Err(invalid()),
            _ => None,
        };
        let Some(key) = key else {
            retained.push(part);
            continue;
        };
        if part.content_type.is_some() || part.file_name.is_some() || part.bytes.is_empty() {
            return Err(invalid());
        }
        let value = String::from_utf8(part.bytes).map_err(|_| invalid())?;
        if metadata
            .insert(key.to_owned(), serde_json::Value::String(value))
            .is_some()
        {
            return Err(invalid());
        }
    }
    if metadata.is_empty() {
        *parts = retained;
        return Ok(());
    }
    retained.push(RawPart {
        name: METADATA_PART.to_owned(),
        file_name: None,
        content_type: Some("application/json".to_owned()),
        bytes: serde_json::to_vec(&metadata).map_err(|_| invalid())?,
    });
    *parts = retained;
    Ok(())
}

fn parse_parts(
    mut parts: Vec<RawPart>,
    limits: BundleLimits,
) -> Result<ParsedWorkerUpload, PlatformError> {
    if parts
        .iter()
        .filter(|part| part.name == METADATA_PART)
        .count()
        != 1
    {
        return Err(invalid());
    }
    let metadata_index = parts
        .iter()
        .position(|part| part.name == METADATA_PART)
        .ok_or_else(invalid)?;
    let metadata_part = parts.remove(metadata_index);
    if !matches!(
        metadata_part.content_type.as_deref(),
        None | Some("application/json")
    ) || metadata_part.bytes.is_empty()
        || metadata_part.bytes.len() > MAX_METADATA_BYTES
    {
        return Err(invalid());
    }
    let metadata: WorkerUploadMetadata =
        serde_json::from_slice(&metadata_part.bytes).map_err(|_| invalid())?;
    validate_metadata(&metadata)?;
    let entrypoint = match (&metadata.main_module, &metadata.body_part) {
        (Some(main), None) => Some((main.as_str(), ModuleType::EsModule)),
        (None, Some(main)) => Some((main.as_str(), ModuleType::CommonJsModule)),
        (None, None) if metadata.assets.is_some() => None,
        _ => return Err(invalid()),
    };
    if entrypoint.is_none() {
        if !parts.is_empty() {
            return Err(invalid());
        }
        return Ok(ParsedWorkerUpload {
            metadata,
            bundle: None,
        });
    }
    let (main_module, expected_type) = entrypoint.ok_or_else(invalid)?;
    let mut referenced_parts = std::collections::BTreeMap::new();
    for (part, module_type) in metadata
        .bindings
        .iter()
        .filter_map(|binding| binding.part())
    {
        if referenced_parts.insert(part, module_type).is_some() {
            return Err(invalid());
        }
    }
    let mut modules = Vec::with_capacity(parts.len());
    for part in parts {
        let module_type = module_type(part.content_type.as_deref())?;
        if part.name == main_module && module_type != expected_type {
            return Err(invalid());
        }
        if expected_type == ModuleType::CommonJsModule
            && part.name != main_module
            && module_type != ModuleType::SourceMap
            && referenced_parts.get(part.name.as_str()) != Some(&module_type)
        {
            return Err(invalid());
        }
        modules.push(ModuleInput {
            name: part.name,
            module_type,
            bytes: part.bytes,
        });
    }
    let bundle = CanonicalBundle::build(main_module, modules, limits)?.into_bytes();
    Ok(ParsedWorkerUpload {
        metadata,
        bundle: Some(bundle),
    })
}

fn validate_metadata(metadata: &WorkerUploadMetadata) -> Result<(), PlatformError> {
    if !supports_worker_compatibility(&metadata.compatibility_date, &metadata.compatibility_flags) {
        return Err(PlatformError::new(
            ErrorCode::BundleInvalid,
            "Worker compatibility metadata is unsupported by the pinned runtime",
        ));
    }
    if metadata.bindings.len() > 128
        || metadata.keep_bindings.len() > 32
        || metadata.annotations.len() > 16
    {
        return Err(too_large());
    }
    if let Some(exports) = &metadata.exports {
        for (name, export) in exports {
            if name != "default" {
                validate_binding_name(name)?;
            }
            let _ = export;
        }
    }
    let mut names = BTreeSet::new();
    for binding in &metadata.bindings {
        validate_binding_name(binding.name())?;
        if !names.insert(binding.name()) || binding.has_unsupported_options() {
            return Err(invalid());
        }
    }
    for kind in &metadata.keep_bindings {
        if !supported_inherited_binding_kind(kind) {
            return Err(invalid());
        }
    }
    if metadata.annotations.iter().any(|(name, value)| {
        !matches!(name.as_str(), "workers/tag" | "workers/message")
            || value.is_empty()
            || value.len() > 1_000
            || value.chars().any(char::is_control)
    }) {
        return Err(PlatformError::new(
            ErrorCode::BindingCapabilityUnsupported,
            "Worker Version annotations are unsupported",
        ));
    }
    if let Some(assets) = &metadata.assets {
        if assets.jwt.is_empty() || assets.jwt.len() > 16 * 1024 {
            return Err(invalid());
        }
        validate_assets_config(&assets.config)?;
    }
    if metadata
        .observability
        .as_ref()
        .is_some_and(|observability| observability.enabled)
    {
        return Err(PlatformError::new(
            ErrorCode::BindingCapabilityUnsupported,
            "Cloudflare-hosted Worker observability is unsupported",
        ));
    }
    Ok(())
}

fn validate_assets_config(
    config: &super::model::WorkerUploadAssetsConfig,
) -> Result<(), PlatformError> {
    if config.html_handling.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "auto-trailing-slash" | "force-trailing-slash" | "drop-trailing-slash" | "none"
        )
    }) || config
        .not_found_handling
        .as_deref()
        .is_some_and(|value| !matches!(value, "none" | "404-page" | "single-page-application"))
        || config
            ._redirects
            .as_ref()
            .is_some_and(|value| value.len() > 2 * 1024 * 1024)
        || config
            ._headers
            .as_ref()
            .is_some_and(|value| value.len() > 2 * 1024 * 1024)
    {
        return Err(invalid());
    }
    match &config.run_worker_first {
        None | Some(serde_json::Value::Bool(_)) => Ok(()),
        Some(serde_json::Value::Array(values))
            if values.len() <= 256
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| value.len() <= 1_024)) =>
        {
            Ok(())
        }
        Some(_) => Err(invalid()),
    }
}

fn module_type(content_type: Option<&str>) -> Result<ModuleType, PlatformError> {
    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    match content_type {
        Some("application/javascript+module") => Ok(ModuleType::EsModule),
        Some("application/javascript") => Ok(ModuleType::CommonJsModule),
        Some("application/wasm") => Ok(ModuleType::Wasm),
        Some("application/octet-stream") => Ok(ModuleType::Data),
        Some("application/source-map") => Ok(ModuleType::SourceMap),
        Some("text/plain") => Ok(ModuleType::Text),
        Some("application/json") => Ok(ModuleType::Json),
        _ => Err(invalid()),
    }
}

fn validate_part_name(name: &str) -> Result<(), PlatformError> {
    if name.is_empty()
        || name.len() > 1_024
        || name.starts_with('/')
        || name.starts_with("//")
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
        || name.chars().any(char::is_control)
        || name
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(invalid());
    }
    Ok(())
}

fn supported_inherited_binding_kind(kind: &str) -> bool {
    matches!(
        kind,
        "plain_text"
            | "json"
            | "secret_text"
            | "secret_key"
            | "kv_namespace"
            | "r2_bucket"
            | "d1"
            | "vectorize"
            | "ai_search_namespace"
            | "ai_search"
            | "ai"
            | "durable_object_namespace"
            | "queue"
            | "workflow"
            | "service"
            | "images"
            | "version_metadata"
            | "assets"
            | "wasm_module"
            | "text_blob"
            | "data_blob"
    )
}

fn validate_binding_name(name: &str) -> Result<(), PlatformError> {
    let valid = !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
        && name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'));
    if valid { Ok(()) } else { Err(invalid()) }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleInvalid,
        "Worker multipart upload is invalid",
    )
}

fn too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleTooLarge,
        "Worker multipart upload exceeds limits",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, content_type: &str, bytes: &[u8]) -> RawPart {
        RawPart {
            name: name.to_owned(),
            file_name: None,
            content_type: Some(content_type.to_owned()),
            bytes: bytes.to_vec(),
        }
    }

    fn string_part(name: &str, bytes: &[u8]) -> RawPart {
        RawPart {
            name: name.to_owned(),
            file_name: None,
            content_type: None,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn normalizes_fixed_cloudflare_sdk_metadata_and_file_fields() {
        let mut parts = vec![
            string_part("metadata[main_module]", b"index.js"),
            string_part("metadata[compatibility_date]", b"2026-08-30"),
            RawPart {
                name: "files[]".to_owned(),
                file_name: Some("index.js".to_owned()),
                content_type: Some("application/javascript+module".to_owned()),
                bytes: b"export default {}".to_vec(),
            },
        ];
        normalize_sdk_parts(&mut parts).unwrap();
        let parsed = parse_parts(parts, BundleLimits::default()).unwrap();
        assert_eq!(parsed.metadata.main_module.as_deref(), Some("index.js"));
        assert!(parsed.bundle.is_some());
    }

    #[tokio::test]
    async fn recovers_fixed_cloudflare_sdk_form_boundary_from_body() {
        use axum::extract::FromRequest as _;

        let boundary = "----WebKitFormBoundary00112233445566778899aabbccddeeff";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata[main_module]\"\r\n\r\nindex.js\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"metadata[compatibility_date]\"\r\n\r\n2026-08-30\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files[]\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport default {{}}\r\n--{boundary}--\r\n"
        );
        let request = Request::builder()
            .header(header::CONTENT_TYPE, "application/javascript")
            .body(Body::from(body))
            .unwrap();
        let request = normalize_sdk_multipart_request(request).await.unwrap();
        assert_eq!(
            request.headers()[header::CONTENT_TYPE],
            format!("multipart/form-data; boundary={boundary}")
        );
        let multipart = Multipart::from_request(request, &()).await.unwrap();
        let parsed = parse_worker_upload(multipart, BundleLimits::default())
            .await
            .unwrap();
        assert_eq!(parsed.metadata.main_module.as_deref(), Some("index.js"));
    }

    #[test]
    fn parses_exact_pinned_compatibility_metadata_and_modules() {
        let parsed = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"main_module":"src/index.js","compatibility_date":"2026-08-30","bindings":[{"name":"MODE","type":"plain_text","text":"production"}]}"#,
                ),
                part(
                    "src/index.js",
                    "application/javascript+module",
                    b"export default { fetch() {} };",
                ),
                part("data.txt", "text/plain", b"hello"),
            ],
            BundleLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.metadata.main_module.as_deref(), Some("src/index.js"));
        let bundle =
            CanonicalBundle::parse(parsed.bundle.unwrap(), BundleLimits::default()).unwrap();
        assert_eq!(bundle.manifest().modules.len(), 2);
    }

    #[test]
    fn commonjs_accepts_only_referenced_blobs_and_source_maps() {
        let parsed = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"body_part":"index.js","compatibility_date":"2026-08-30","bindings":[{"name":"MODEL","type":"wasm_module","part":"model.wasm"},{"name":"COPY","type":"text_blob","part":"copy.txt"},{"name":"DATA","type":"data_blob","part":"data.bin"}]}"#,
                ),
                part("index.js", "application/javascript", b"addEventListener('fetch', () => {});") ,
                part("model.wasm", "application/wasm", b"wasm"),
                part("copy.txt", "text/plain", b"hello"),
                part("data.bin", "application/octet-stream", b"bytes"),
                part("index.js.map", "application/source-map", br#"{"version":3}"#),
            ],
            BundleLimits::default(),
        )
        .unwrap();
        let bundle =
            CanonicalBundle::parse(parsed.bundle.unwrap(), BundleLimits::default()).unwrap();
        assert_eq!(bundle.manifest().modules.len(), 5);

        let error = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"body_part":"index.js","compatibility_date":"2026-08-30"}"#,
                ),
                part(
                    "index.js",
                    "application/javascript",
                    b"addEventListener('fetch', () => {});",
                ),
                part(
                    "extra.js",
                    "application/javascript",
                    b"module.exports = {};",
                ),
            ],
            BundleLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::BundleInvalid);
    }

    #[test]
    fn rejects_ambiguous_blob_references_and_windows_paths() {
        for name in [
            "..\\secret",
            "C:\\worker.js",
            "\\\\server\\share",
            "dir\\worker.js",
        ] {
            let error = validate_part_name(name).unwrap_err();
            assert_eq!(error.code(), ErrorCode::BundleInvalid);
        }

        let error = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"body_part":"index.js","compatibility_date":"2026-08-30","bindings":[{"name":"A","type":"text_blob","part":"copy.txt"},{"name":"B","type":"text_blob","part":"copy.txt"}]}"#,
                ),
                part("index.js", "application/javascript", b"addEventListener('fetch', () => {});") ,
                part("copy.txt", "text/plain", b"hello"),
            ],
            BundleLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::BundleInvalid);
    }

    #[test]
    fn accepts_the_fixed_wrangler_keep_bindings_inventory() {
        for kind in [
            "plain_text",
            "json",
            "secret_text",
            "secret_key",
            "kv_namespace",
            "r2_bucket",
            "d1",
            "vectorize",
            "ai_search_namespace",
            "ai_search",
            "ai",
            "durable_object_namespace",
            "queue",
            "workflow",
            "service",
            "images",
            "version_metadata",
            "assets",
            "wasm_module",
            "text_blob",
            "data_blob",
        ] {
            assert!(supported_inherited_binding_kind(kind), "{kind}");
        }
        assert!(!supported_inherited_binding_kind("unsafe_unknown_kind"));
    }

    #[test]
    fn accepts_commonjs_body_part_and_assets_only_uploads() {
        let commonjs = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"body_part":"index.js","compatibility_date":"2026-08-30"}"#,
                ),
                part(
                    "index.js",
                    "application/javascript",
                    b"addEventListener('fetch',()=>{})",
                ),
            ],
            BundleLimits::default(),
        )
        .unwrap();
        assert!(commonjs.bundle.is_some());

        let assets = parse_parts(
            vec![part(
                "metadata",
                "application/json",
                br#"{"compatibility_date":"2026-08-30","assets":{"jwt":"completion-token","config":{}}}"#,
            )],
            BundleLimits::default(),
        )
        .unwrap();
        assert!(assets.bundle.is_none());
    }

    #[test]
    fn accepts_redundant_node_flag_and_rejects_other_compatibility_metadata() {
        assert!(
            parse_parts(
                vec![
                    part(
                        "metadata",
                        "application/json",
                        br#"{"main_module":"index.js","compatibility_date":"2026-08-30","compatibility_flags":["nodejs_compat"]}"#,
                    ),
                    part(
                        "index.js",
                        "application/javascript+module",
                        b"export default {}",
                    ),
                ],
                BundleLimits::default(),
            )
            .is_ok()
        );
        for metadata in [
            br#"{"main_module":"index.js","compatibility_date":"2026-08-29"}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-08-30","compatibility_flags":["nodejs_compat_v2"]}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-08-30","compatibility_flags":["nodejs_compat","nodejs_compat"]}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-08-30","limits":{"cpu_ms":10}}"#.as_slice(),
        ] {
            assert!(parse_parts(
                vec![
                    part("metadata", "application/json", metadata),
                    part("index.js", "application/javascript+module", b"export default {}"),
                ],
                BundleLimits::default(),
            )
            .is_err());
        }
        assert!(
            parse_parts(
                vec![
                    part(
                        "metadata",
                        "application/json",
                        br#"{"main_module":"../index.js","compatibility_date":"2026-08-30"}"#,
                    ),
                    part(
                        "../index.js",
                        "application/javascript+module",
                        b"export default {}"
                    ),
                ],
                BundleLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_binding_names_and_unsupported_mime_types() {
        assert!(parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"main_module":"index.py","compatibility_date":"2026-08-30","bindings":[{"name":"DUP","type":"plain_text","text":"a"},{"name":"DUP","type":"json","json":1}]}"#,
                ),
                part("index.py", "text/x-python", b"print('no')"),
            ],
            BundleLimits::default(),
        )
        .is_err());
    }

    #[test]
    fn accepts_wrangler_javascript_identifier_binding_names() {
        for name in ["_PRIVATE", "$service"] {
            let metadata = format!(
                r#"{{"main_module":"index.js","compatibility_date":"2026-08-30","bindings":[{{"name":"{name}","type":"plain_text","text":"ok"}}]}}"#
            );
            assert!(
                parse_parts(
                    vec![
                        string_part("metadata", metadata.as_bytes()),
                        part(
                            "index.js",
                            "application/javascript+module",
                            b"export default {}",
                        ),
                    ],
                    BundleLimits::default(),
                )
                .is_ok(),
                "Wrangler binding name {name} must be accepted"
            );
        }
    }

    #[test]
    fn metadata_part_has_exact_fixed_wrangler_mime_and_is_unique() {
        let metadata = br#"{"main_module":"index.js","compatibility_date":"2026-08-30"}"#;
        assert!(
            parse_parts(
                vec![
                    string_part("metadata", metadata),
                    part(
                        "index.js",
                        "application/javascript+module",
                        b"export default {}",
                    ),
                ],
                BundleLimits::default(),
            )
            .is_ok(),
            "Undici FormData emits Wrangler's metadata string without a Content-Type"
        );
        for content_type in ["text/plain", "application/json; charset=utf-8"] {
            assert!(
                parse_parts(
                    vec![
                        part("metadata", content_type, metadata),
                        part(
                            "index.js",
                            "application/javascript+module",
                            b"export default {}",
                        ),
                    ],
                    BundleLimits::default(),
                )
                .is_err()
            );
        }
        assert!(
            parse_parts(
                vec![
                    part("metadata", "application/json", metadata),
                    part("metadata", "application/json", metadata),
                    part(
                        "index.js",
                        "application/javascript+module",
                        b"export default {}",
                    ),
                ],
                BundleLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn fixed_wrangler_deprecated_queue_delay_metadata_is_accepted() {
        let parsed = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"main_module":"index.js","compatibility_date":"2026-08-30","bindings":[{"name":"EVENTS","type":"queue","queue_name":"events","delivery_delay":60}]}"#,
                ),
                part(
                    "index.js",
                    "application/javascript+module",
                    b"export default {}",
                ),
            ],
            BundleLimits::default(),
        )
        .unwrap();
        let [
            super::super::model::WorkerUploadBinding::Queue {
                _delivery_delay: Some(delay),
                ..
            },
        ] = parsed.metadata.bindings.as_slice()
        else {
            panic!("fixed Wrangler Queue binding was not parsed");
        };
        assert_eq!(delay.as_u64(), Some(60));
    }
}
