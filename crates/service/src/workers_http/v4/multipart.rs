//! Bounded streaming parser for Cloudflare Worker multipart uploads.

use super::model::WorkerUploadMetadata;
use axum::extract::Multipart;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, ModuleInput, ModuleType, supports_worker_compatibility,
};
use std::collections::BTreeSet;

const METADATA_PART: &str = "metadata";
pub(super) const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub(super) const MAX_SDK_METADATA_FIELDS: usize = 2_048;
pub(super) const MAX_SDK_FIELD_NAME_BYTES: usize = 4 * 1024;
pub(super) const MAX_BOUNDARY_BYTES: usize = 70;
const MAX_MODULE_PART_NAME_BYTES: usize = 1_024;
const MAX_PART_FIXED_WIRE_BYTES: usize = 512;
const MAX_UPLOAD_PARTS: usize = MAX_SDK_METADATA_FIELDS + BundleLimits::DEFAULT.max_modules;
const MAX_MULTIPART_WIRE_OVERHEAD: usize = MAX_SDK_METADATA_FIELDS * MAX_SDK_FIELD_NAME_BYTES
    + BundleLimits::DEFAULT.max_modules * MAX_MODULE_PART_NAME_BYTES * 2
    + MAX_UPLOAD_PARTS * (MAX_PART_FIXED_WIRE_BYTES + MAX_BOUNDARY_BYTES)
    + MAX_BOUNDARY_BYTES;

/// Maximum complete Worker upload wire body accepted by the fixed P6 surface.
pub(super) const MAX_BODY_BYTES: usize =
    BundleLimits::DEFAULT.max_total_module_bytes + MAX_METADATA_BYTES + MAX_MULTIPART_WIRE_OVERHEAD;

#[derive(Clone, Debug)]
pub(super) struct RawPart {
    pub(super) name: String,
    pub(super) file_name: Option<String>,
    pub(super) content_type: Option<String>,
    pub(super) bytes: Vec<u8>,
}

/// Fully validated upload ready for the immutable Version pipeline.
#[derive(Clone, Debug)]
pub(crate) struct ParsedWorkerUpload {
    /// Closed metadata emitted by the pinned Wrangler multipart generator.
    pub metadata: WorkerUploadMetadata,
    /// Canonical Worker bundle bytes, absent only for an assets-only Version.
    pub bundle: Option<Vec<u8>>,
}

/// Incrementally read and bound a Worker multipart request.
pub(crate) async fn parse_worker_upload(
    mut multipart: Multipart,
    limits: BundleLimits,
) -> Result<ParsedWorkerUpload, PlatformError> {
    let mut module_total = 0_usize;
    let mut metadata_total = 0_usize;
    let mut module_count = 0_usize;
    let mut metadata_field_count = 0_usize;
    let mut names = BTreeSet::new();
    let mut parts = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(|_| invalid())? {
        let name = field.name().ok_or_else(invalid)?.to_owned();
        let is_metadata = name == METADATA_PART || name.starts_with("metadata[");
        if is_metadata {
            metadata_field_count = metadata_field_count.checked_add(1).ok_or_else(too_large)?;
            if metadata_field_count > MAX_SDK_METADATA_FIELDS {
                return Err(too_large());
            }
            super::sdk_multipart::validate_metadata_field_name(&name)?;
        } else {
            module_count = module_count.checked_add(1).ok_or_else(too_large)?;
            if module_count > limits.max_modules {
                return Err(too_large());
            }
            validate_part_name(&name)?;
        }
        let file_name = field.file_name().map(ToOwned::to_owned);
        if file_name.as_ref().is_some_and(|value| {
            value.len() > MAX_MODULE_PART_NAME_BYTES || value.chars().any(char::is_control)
        }) {
            return Err(invalid());
        }
        let header_bytes = field
            .headers()
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total
                    .checked_add(name.as_str().len())
                    .and_then(|total| total.checked_add(value.as_bytes().len()))
                    .and_then(|total| total.checked_add(4))
            });
        let variable_header_bytes = if is_metadata {
            MAX_SDK_FIELD_NAME_BYTES
        } else {
            MAX_MODULE_PART_NAME_BYTES * 2
        };
        if header_bytes
            .is_none_or(|value| value > MAX_PART_FIXED_WIRE_BYTES + variable_header_bytes)
        {
            return Err(too_large());
        }
        let content_type = field.content_type().map(ToOwned::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|_| invalid())? {
            let next_part = bytes.len().checked_add(chunk.len()).ok_or_else(too_large)?;
            let aggregate = if is_metadata {
                &mut metadata_total
            } else {
                &mut module_total
            };
            *aggregate = aggregate.checked_add(chunk.len()).ok_or_else(too_large)?;
            let aggregate_limit = if is_metadata {
                MAX_METADATA_BYTES
            } else {
                limits.max_total_module_bytes
            };
            let part_limit = if is_metadata {
                MAX_METADATA_BYTES
            } else {
                limits.max_module_bytes
            };
            if *aggregate > aggregate_limit || next_part > part_limit {
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
    super::sdk_multipart::normalize_parts(&mut parts)?;
    for part in &parts {
        if !names.insert(part.name.clone()) {
            return Err(invalid());
        }
    }
    parse_parts(parts, limits)
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
    if let Some(observability) = &metadata.observability {
        validate_sampling_rate(observability.head_sampling_rate)?;
        if let Some(logs) = &observability.logs {
            validate_sampling_rate(logs.head_sampling_rate)?;
            if !logs.destinations.is_empty() {
                return Err(PlatformError::new(
                    ErrorCode::BindingCapabilityUnsupported,
                    "Workers Logs destinations are unsupported",
                ));
            }
        }
        if let Some(traces) = &observability.traces
            && (traces.enabled.is_some_and(|value| value)
                || traces.persist.is_some_and(|value| value)
                || traces.head_sampling_rate.is_some()
                || !traces.destinations.is_empty())
        {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "Workers trace persistence is unsupported",
            ));
        }
    }
    Ok(())
}

fn validate_sampling_rate(value: Option<f64>) -> Result<(), PlatformError> {
    if value.is_none_or(|rate| rate.is_finite() && (0.0..=1.0).contains(&rate)) {
        Ok(())
    } else {
        Err(invalid())
    }
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

pub(super) fn validate_part_name(name: &str) -> Result<(), PlatformError> {
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

pub(super) fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleInvalid,
        "Worker multipart upload is invalid",
    )
}

pub(super) fn too_large() -> PlatformError {
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
