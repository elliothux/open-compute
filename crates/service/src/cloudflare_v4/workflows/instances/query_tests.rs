use super::*;

fn request(query: &str) -> Request {
    let uri = if query.is_empty() {
        "/".to_owned()
    } else {
        format!("/?{query}")
    };
    Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

#[test]
fn list_and_detail_queries_cover_defaults_filters_and_rejections() {
    let defaults = ListQuery::parse(&request("")).unwrap();
    assert_eq!(defaults.page, 1);
    assert_eq!(defaults.per_page, 50);
    assert!(defaults.descending());
    assert_eq!(defaults.binding(), "50|desc|||");
    let filtered = ListQuery::parse(&request(
        "per_page=100&cursor=next&direction=asc&status=running&date_start=1970-01-01T00%3A00%3A00Z&date_end=1970-01-01T00%3A00%3A01Z",
    ))
    .unwrap();
    assert_eq!(filtered.cursor.as_deref(), Some("next"));
    assert!(!filtered.descending());
    assert_eq!(filtered.binding(), "100|asc|running|0|1000");

    for query in [
        "unknown=true",
        "page=0",
        "per_page=101",
        "page=1&cursor=next",
        "direction=sideways",
        "status=unknown",
        "date_start=invalid",
        "date_start=1970-01-02T00%3A00%3A00Z&date_end=1970-01-01T00%3A00%3A00Z",
    ] {
        assert!(ListQuery::parse(&request(query)).is_err(), "{query}");
    }

    let detail = DetailQuery::parse(&request("simple=true&order=desc")).unwrap();
    assert!(detail.simple);
    assert!(matches!(detail.order, Direction::Desc));
    for query in ["simple=maybe", "order=sideways", "unknown=true"] {
        assert!(DetailQuery::parse(&request(query)).is_err(), "{query}");
    }
}

#[test]
fn status_actions_cover_pause_resume_terminate_rollback_and_restart() {
    for (value, expected) in [
        (serde_json::json!({"status":"pause"}), "modify"),
        (serde_json::json!({"status":"resume"}), "modify"),
        (serde_json::json!({"status":"terminate"}), "modify"),
        (
            serde_json::json!({"status":"terminate","rollback":true}),
            "rollback",
        ),
        (serde_json::json!({"status":"restart"}), "restart"),
        (
            serde_json::json!({"status":"restart","from":{"name":"step","count":2,"type":"do"}}),
            "restart",
        ),
    ] {
        let body: StatusBody = serde_json::from_value(value).unwrap();
        let action = body.validate().unwrap();
        let actual = match action {
            StatusAction::Modify(_) => "modify",
            StatusAction::Rollback => "rollback",
            StatusAction::Restart(selector) => {
                if let Some(selector) = selector {
                    assert!(!selector.name.is_empty());
                }
                "restart"
            }
        };
        assert_eq!(actual, expected);
    }
    for value in [
        serde_json::json!({"status":"unknown"}),
        serde_json::json!({"status":"pause","rollback":true}),
        serde_json::json!({"status":"resume","from":{"name":"step"}}),
        serde_json::json!({"status":"restart","rollback":false}),
        serde_json::json!({"status":"restart","from":{"name":"","count":0}}),
    ] {
        let body: StatusBody = serde_json::from_value(value).unwrap();
        assert!(body.validate().is_err());
    }
}
