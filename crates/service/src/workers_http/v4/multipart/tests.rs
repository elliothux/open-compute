use super::*;

#[test]
fn official_text_javascript_parts_keep_module_semantics() {
    assert_eq!(
        module_type(Some("text/javascript+module; charset=utf-8")).unwrap(),
        ModuleType::EsModule
    );
    assert_eq!(
        module_type(Some("text/javascript; charset=utf-8")).unwrap(),
        ModuleType::CommonJsModule
    );
    assert!(module_type(Some("text/javascript+unknown")).is_err());
}

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
                    br#"{"main_module":"src/index.js","compatibility_date":"2026-09-08","bindings":[{"name":"MODE","type":"plain_text","text":"production"}]}"#,
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
    let bundle = CanonicalBundle::parse(parsed.bundle.unwrap(), BundleLimits::default()).unwrap();
    assert_eq!(bundle.manifest().modules.len(), 2);
}

#[test]
fn commonjs_accepts_only_referenced_blobs_and_source_maps() {
    let parsed = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"body_part":"index.js","compatibility_date":"2026-09-08","bindings":[{"name":"MODEL","type":"wasm_module","part":"model.wasm"},{"name":"COPY","type":"text_blob","part":"copy.txt"},{"name":"DATA","type":"data_blob","part":"data.bin"}]}"#,
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
    let bundle = CanonicalBundle::parse(parsed.bundle.unwrap(), BundleLimits::default()).unwrap();
    assert_eq!(bundle.manifest().modules.len(), 5);

    let error = parse_parts(
        vec![
            part(
                "metadata",
                "application/json",
                br#"{"body_part":"index.js","compatibility_date":"2026-09-08"}"#,
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
                    br#"{"body_part":"index.js","compatibility_date":"2026-09-08","bindings":[{"name":"A","type":"text_blob","part":"copy.txt"},{"name":"B","type":"text_blob","part":"copy.txt"}]}"#,
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
                br#"{"body_part":"index.js","compatibility_date":"2026-09-08"}"#,
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
                br#"{"compatibility_date":"2026-09-08","assets":{"jwt":"completion-token","config":{}}}"#,
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
                        br#"{"main_module":"index.js","compatibility_date":"2026-09-08","compatibility_flags":["nodejs_compat"]}"#,
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
    let limited = parse_parts(
            vec![
                part(
                    "metadata",
                    "application/json",
                    br#"{"main_module":"index.js","compatibility_date":"2026-09-08","limits":{"cpu_ms":10,"subrequests":20}}"#,
                ),
                part("index.js", "application/javascript+module", b"export default {}"),
            ],
            BundleLimits::default(),
        )
        .unwrap();
    assert_eq!(limited.metadata.limits.unwrap().sub_requests, Some(20));
    for metadata in [
            br#"{"main_module":"index.js","compatibility_date":"2026-08-29"}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-09-08","compatibility_flags":["nodejs_compat_v2"]}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-09-08","compatibility_flags":["nodejs_compat","nodejs_compat"]}"#.as_slice(),
            br#"{"main_module":"index.js","compatibility_date":"2026-09-08","limits":{"cpuMs":10}}"#.as_slice(),
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
                    br#"{"main_module":"../index.js","compatibility_date":"2026-09-08"}"#,
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
                    br#"{"main_module":"index.py","compatibility_date":"2026-09-08","bindings":[{"name":"DUP","type":"plain_text","text":"a"},{"name":"DUP","type":"json","json":1}]}"#,
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
            r#"{{"main_module":"index.js","compatibility_date":"2026-09-08","bindings":[{{"name":"{name}","type":"plain_text","text":"ok"}}]}}"#
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
    let metadata = br#"{"main_module":"index.js","compatibility_date":"2026-09-08"}"#;
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
                    br#"{"main_module":"index.js","compatibility_date":"2026-09-08","bindings":[{"name":"EVENTS","type":"queue","queue_name":"events","delivery_delay":60}]}"#,
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
