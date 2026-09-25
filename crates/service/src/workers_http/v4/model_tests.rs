use super::*;

#[test]
fn upload_metadata_debug_and_binding_helpers_cover_the_closed_wire_union() {
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module":"index.js",
        "compatibility_date":"2026-09-08",
        "compatibility_flags":["nodejs_compat"],
        "package_dependencies":[{
            "name":"hono",
            "packageJsonVersion":"^4.0.0",
            "installedVersion":"4.9.0"
        }],
        "bindings":[],
        "keep_bindings":["plain_text"],
        "annotations":{"message":"release"},
        "assets":{"jwt":"ticket","config":{}},
        "observability":{"enabled":true},
        "cache_options":{"enabled":true},
        "exports":{"default":{"type":"worker","cache":{"enabled":true}}},
        "migrations":{"new_tag":"v1","steps":[]}
    }))
    .unwrap();
    let debug = format!("{metadata:?}");
    for expected in [
        "index.js",
        "nodejs_compat",
        "package_dependencies: 1",
        "has_assets: true",
        "has_observability: true",
        "has_cache_options: true",
        "has_exports: true",
        "has_migrations: true",
    ] {
        assert!(debug.contains(expected), "{debug}");
    }

    let bindings: Vec<WorkerUploadBinding> = serde_json::from_value(serde_json::json!([
        {"type":"plain_text","name":"plain","text":"value"},
        {"type":"json","name":"json","json":{"value":1}},
        {"type":"secret_text","name":"secret","text":"hidden"},
        {"type":"kv_namespace","name":"kv","namespace_id":"id"},
        {"type":"r2_bucket","name":"r2","bucket_name":"bucket"},
        {"type":"d1","name":"d1","database_id":"id"},
        {"type":"vectorize","name":"vector","index_name":"index"},
        {"type":"ai_search_namespace","name":"search-ns","namespace":"namespace"},
        {"type":"ai_search","name":"search","instance_name":"instance"},
        {"type":"ai","name":"ai"},
        {"type":"durable_object_namespace","name":"do","class_name":"Object"},
        {"type":"queue","name":"queue","queue_name":"queue"},
        {"type":"workflow","name":"workflow","workflow_name":"workflow"},
        {"type":"service","name":"service","service":"target"},
        {"type":"images","name":"images"},
        {"type":"version_metadata","name":"version"},
        {"type":"assets","name":"assets"},
        {"type":"wasm_module","name":"wasm","part":"wasm-part"},
        {"type":"text_blob","name":"text","part":"text-part"},
        {"type":"data_blob","name":"data","part":"data-part"},
        {"type":"inherit","name":"inherit"}
    ]))
    .unwrap();
    assert_eq!(bindings.iter().map(WorkerUploadBinding::name).count(), 21);
    assert!(bindings.iter().all(|binding| !binding.name().is_empty()));
    assert_eq!(
        bindings[17].part(),
        Some(("wasm-part", open_compute_workers::ModuleType::Wasm))
    );
    assert_eq!(
        bindings[18].part(),
        Some(("text-part", open_compute_workers::ModuleType::Text))
    );
    assert_eq!(
        bindings[19].part(),
        Some(("data-part", open_compute_workers::ModuleType::Data))
    );
    assert!(bindings[0].part().is_none());
    assert!(
        bindings
            .iter()
            .all(|binding| !binding.has_unsupported_options())
    );
}

#[test]
fn d1_binding_accepts_both_official_identifiers_but_rejects_ambiguity() {
    for key in ["database_id", "id"] {
        let mut binding = serde_json::json!({"type":"d1","name":"DB"});
        binding[key] = serde_json::json!("db-id");
        assert!(serde_json::from_value::<WorkerUploadBinding>(binding).is_ok());
    }
    assert!(
        serde_json::from_value::<WorkerUploadBinding>(serde_json::json!({
            "type":"d1","name":"DB","database_id":"new","id":"old"
        }))
        .is_err()
    );
}

#[test]
fn package_dependencies_accepts_only_the_wrangler_wire_shape() {
    for package_dependencies in [
        serde_json::json!([]),
        serde_json::json!([{
            "name":"hono",
            "packageJsonVersion":"^4.0.0",
            "installedVersion":"4.9.0"
        }]),
    ] {
        assert!(
            serde_json::from_value::<WorkerUploadMetadata>(serde_json::json!({
                "main_module":"index.js",
                "compatibility_date":"2026-09-08",
                "package_dependencies":package_dependencies
            }))
            .is_ok()
        );
    }

    for package_dependencies in [
        serde_json::json!(null),
        serde_json::json!([{"name":"hono"}]),
        serde_json::json!([{
            "name":"hono",
            "packageJsonVersion":"^4.0.0",
            "installedVersion":"4.9.0",
            "unknown":true
        }]),
    ] {
        assert!(
            serde_json::from_value::<WorkerUploadMetadata>(serde_json::json!({
                "main_module":"index.js",
                "compatibility_date":"2026-09-08",
                "package_dependencies":package_dependencies
            }))
            .is_err()
        );
    }
}

#[test]
fn unsupported_binding_options_are_rejected_by_every_affected_variant() {
    let bindings: Vec<WorkerUploadBinding> = serde_json::from_value(serde_json::json!([
        {"type":"kv_namespace","name":"kv","namespace_id":"id","raw":true},
        {"type":"vectorize","name":"vector","index_name":"index","raw":false},
        {"type":"r2_bucket","name":"r2","bucket_name":"bucket","jurisdiction":"eu"},
        {"type":"d1","name":"d1","database_id":"id","internalEnv":"preview"},
        {"type":"ai","name":"ai","staging":true},
        {"type":"queue","name":"queue","queue_name":"queue","raw":true},
        {"type":"workflow","name":"workflow","workflow_name":"workflow","raw":true},
        {"type":"durable_object_namespace","name":"do","class_name":"Object","environment":"preview"},
        {"type":"service","name":"service","service":"target","cross_account_grant":"grant"}
    ]))
    .unwrap();
    assert!(
        bindings
            .iter()
            .all(WorkerUploadBinding::has_unsupported_options)
    );
}

#[test]
fn resource_limits_use_the_wrangler_snake_case_wire_schema() {
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module":"index.js",
        "compatibility_date":"2026-09-08",
        "limits":{"cpu_ms":1234,"subrequests":5678}
    }))
    .unwrap();
    let limits = metadata.limits.unwrap();
    assert_eq!(limits.cpu_ms, Some(1234));
    assert_eq!(limits.sub_requests, Some(5678));

    for limits in [
        serde_json::json!(null),
        serde_json::json!({"cpuMs":1}),
        serde_json::json!({"subRequests":1}),
        serde_json::json!({"cpu_ms":null}),
        serde_json::json!({"subrequests":null}),
        serde_json::json!({"cpu_ms":1.5}),
        serde_json::json!({"cpu_ms":-1}),
        serde_json::json!({"unknown":1}),
    ] {
        assert!(
            serde_json::from_value::<WorkerUploadMetadata>(serde_json::json!({
                "main_module":"index.js",
                "compatibility_date":"2026-09-08",
                "limits":limits
            }))
            .is_err()
        );
    }
}
