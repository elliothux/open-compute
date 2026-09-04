use super::*;

#[test]
fn upload_metadata_debug_and_binding_helpers_cover_the_closed_wire_union() {
    let metadata: WorkerUploadMetadata = serde_json::from_value(serde_json::json!({
        "main_module":"index.js",
        "compatibility_date":"2026-08-30",
        "compatibility_flags":["nodejs_compat"],
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
        {"type":"d1","name":"d1","id":"id"},
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
fn unsupported_binding_options_are_rejected_by_every_affected_variant() {
    let bindings: Vec<WorkerUploadBinding> = serde_json::from_value(serde_json::json!([
        {"type":"kv_namespace","name":"kv","namespace_id":"id","raw":true},
        {"type":"vectorize","name":"vector","index_name":"index","raw":false},
        {"type":"r2_bucket","name":"r2","bucket_name":"bucket","jurisdiction":"eu"},
        {"type":"d1","name":"d1","id":"id","internalEnv":"preview"},
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
