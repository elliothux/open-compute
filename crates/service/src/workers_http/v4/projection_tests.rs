use super::*;

#[test]
fn binding_kinds_and_named_projection_cover_the_closed_day1_set() {
    for (kind, expected) in [
        (BindingKind::KvNamespace, "kv_namespace"),
        (BindingKind::R2Bucket, "r2_bucket"),
        (BindingKind::D1Database, "d1"),
        (BindingKind::DoNamespace, "durable_object_namespace"),
        (BindingKind::VectorizeIndex, "vectorize"),
        (BindingKind::AiSearchNamespace, "ai_search_namespace"),
        (BindingKind::AiSearchInstance, "ai_search"),
        (BindingKind::QueueProducer, "queue"),
        (BindingKind::Workflow, "workflow"),
    ] {
        assert_eq!(wrangler_kind(kind), expected);
    }
    assert_eq!(
        named_binding("BINDING", "r2_bucket", "bucket_name", "assets"),
        serde_json::json!({
            "name":"BINDING",
            "type":"r2_bucket",
            "bucket_name":"assets"
        })
    );
    assert_eq!(invariant().code(), ErrorCode::VersionInvariantViolation);
}
