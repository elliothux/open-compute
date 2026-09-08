use super::*;

#[test]
fn operation_metrics_and_empty_payload_validation_cover_all_categories() {
    for (operation, expected) in [
        ("namespace.search", AiSearchOperation::Search),
        ("instance.search", AiSearchOperation::Search),
        ("namespace.chatCompletions", AiSearchOperation::Chat),
        ("instance.chatCompletions", AiSearchOperation::Chat),
        ("namespace.list", AiSearchOperation::Namespace),
        ("instance.info", AiSearchOperation::Instance),
        ("item.info", AiSearchOperation::Item),
        ("jobs.list", AiSearchOperation::Job),
    ] {
        assert_eq!(metric_operation(operation), expected);
    }
    assert!(require_empty_object(&json!({})).is_ok());
    assert!(require_empty_object(&Value::Null).is_err());
    assert!(require_empty_object(&json!([])).is_err());
    assert!(unix_ms() > 0);
}
