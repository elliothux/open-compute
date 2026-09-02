use crate::catalog_page::{
    CatalogColumns, CatalogCursor, CatalogCursorValue, CatalogDirection, CatalogSort,
    build_catalog_sql, decode_catalog_cursor, decode_created_id_cursor, decode_name_id_cursor,
    encode_catalog_cursor, encode_created_id_cursor, encode_name_id_cursor,
    normalize_catalog_limit, record_catalog_cursor, search_as_queue_id, search_as_resource_id,
    search_as_worker_id, search_as_workflow_id,
};
use open_compute_core::{QueueId, ResourceId, WorkerId, WorkflowId};
use rusqlite::types::Value;
use std::str::FromStr;

const COLUMNS: CatalogColumns<'static> = CatalogColumns {
    id: "id",
    name: "name",
    state: "state",
    created_at: "created_at_ms",
    updated_at: "updated_at_ms",
};

#[test]
fn catalog_tokens_limits_and_typed_searches_are_exact() {
    for (sort, token) in [
        (CatalogSort::Name, "name"),
        (CatalogSort::CreatedAt, "createdAt"),
        (CatalogSort::UpdatedAt, "updatedAt"),
    ] {
        assert_eq!(sort.as_str(), token);
        assert_eq!(CatalogSort::from_str(token).unwrap(), sort);
    }
    assert!(CatalogSort::from_str("created_at").is_err());

    for (direction, token, comparison, sql) in [
        (CatalogDirection::Asc, "asc", ">", "ASC"),
        (CatalogDirection::Desc, "desc", "<", "DESC"),
    ] {
        assert_eq!(CatalogDirection::from_str(token).unwrap(), direction);
        assert_eq!(direction.comparison(), comparison);
        assert_eq!(direction.sql(), sql);
    }
    assert!(CatalogDirection::from_str("sideways").is_err());
    assert_eq!(normalize_catalog_limit(0), 1);
    assert_eq!(normalize_catalog_limit(100), 100);
    assert_eq!(normalize_catalog_limit(u16::MAX), 1_000);

    let resource = ResourceId::generate();
    let worker = WorkerId::generate();
    let queue = QueueId::generate();
    let workflow = WorkflowId::generate();
    assert_eq!(
        search_as_resource_id(&format!(" {resource} ")),
        Some(resource)
    );
    assert_eq!(search_as_worker_id(&format!(" {worker} ")), Some(worker));
    assert_eq!(search_as_queue_id(&format!(" {queue} ")), Some(queue));
    assert_eq!(
        search_as_workflow_id(&format!(" {workflow} ")),
        Some(workflow)
    );
    assert_eq!(search_as_resource_id("display-name"), None);
    assert_eq!(search_as_worker_id("display-name"), None);
    assert_eq!(search_as_queue_id("display-name"), None);
    assert_eq!(search_as_workflow_id("display-name"), None);
}

#[test]
fn catalog_cursor_codecs_reject_malformed_or_confused_values() {
    let resource = ResourceId::generate();
    let name_cursor = encode_name_id_cursor("alpha", resource);
    let decoded = decode_name_id_cursor(&name_cursor).unwrap();
    assert_eq!(decoded.name, "alpha");
    assert_eq!(decoded.id, resource);
    assert!(decode_name_id_cursor("not-base64!").is_err());
    assert!(decode_name_id_cursor("e30").is_err());

    let created = encode_created_id_cursor(42, "stable-id");
    let decoded = decode_created_id_cursor(&created).unwrap();
    assert_eq!(decoded.created_at_ms, 42);
    assert_eq!(decoded.id, "stable-id");
    assert!(decode_created_id_cursor("not-base64!").is_err());
    assert!(decode_created_id_cursor("e30").is_err());

    let cursor = CatalogCursor {
        sort: CatalogSort::UpdatedAt,
        direction: CatalogDirection::Desc,
        value: CatalogCursorValue::Integer(77),
        id: "row-id".to_owned(),
    };
    assert_eq!(
        decode_catalog_cursor(&encode_catalog_cursor(&cursor)).unwrap(),
        cursor
    );
    assert!(decode_catalog_cursor("not-base64!").is_err());
    assert!(decode_catalog_cursor("e30").is_err());
    let empty = CatalogCursor {
        id: String::new(),
        ..cursor
    };
    assert!(decode_catalog_cursor(&encode_catalog_cursor(&empty)).is_err());
}

#[test]
fn catalog_sql_binds_filters_sort_and_cursor_without_interpolation() {
    let base = "SELECT * FROM catalog WHERE account_id = ?";
    let exact = build_catalog_sql(
        base,
        COLUMNS,
        "account".to_owned(),
        Some("ignored-search".to_owned()),
        Some("exact-id".to_owned()),
        Some("ready".to_owned()),
        CatalogSort::Name,
        CatalogDirection::Asc,
        Some(CatalogCursor {
            sort: CatalogSort::Name,
            direction: CatalogDirection::Asc,
            value: CatalogCursorValue::Text("prior".to_owned()),
            id: "prior-id".to_owned(),
        }),
        101,
    )
    .unwrap();
    assert!(exact.text.contains("id = ?"));
    assert!(!exact.text.contains("ignored-search"));
    assert!(exact.text.contains("state = ?"));
    assert!(exact.text.contains("name > ?"));
    assert!(exact.text.ends_with("ORDER BY name ASC, id ASC LIMIT ?"));
    assert_eq!(
        exact.values,
        vec![
            Value::Text("account".to_owned()),
            Value::Text("exact-id".to_owned()),
            Value::Text("ready".to_owned()),
            Value::Text("prior".to_owned()),
            Value::Text("prior".to_owned()),
            Value::Text("prior-id".to_owned()),
            Value::Integer(101),
        ]
    );

    for (sort, expected_order) in [
        (CatalogSort::CreatedAt, "created_at_ms DESC"),
        (CatalogSort::UpdatedAt, "updated_at_ms DESC"),
    ] {
        let query = build_catalog_sql(
            base,
            COLUMNS,
            "account".to_owned(),
            Some("needle".to_owned()),
            None,
            None,
            sort,
            CatalogDirection::Desc,
            Some(CatalogCursor {
                sort,
                direction: CatalogDirection::Desc,
                value: CatalogCursorValue::Integer(123),
                id: "row".to_owned(),
            }),
            2,
        )
        .unwrap();
        assert!(query.text.contains("INSTR(LOWER(name), ?) > 0"));
        assert!(query.text.contains(expected_order));
        assert!(query.values.contains(&Value::Text("needle".to_owned())));
        assert!(query.values.contains(&Value::Integer(123)));
    }

    for cursor in [
        CatalogCursor {
            sort: CatalogSort::Name,
            direction: CatalogDirection::Desc,
            value: CatalogCursorValue::Text("x".to_owned()),
            id: "row".to_owned(),
        },
        CatalogCursor {
            sort: CatalogSort::CreatedAt,
            direction: CatalogDirection::Asc,
            value: CatalogCursorValue::Text("wrong-type".to_owned()),
            id: "row".to_owned(),
        },
    ] {
        assert!(
            build_catalog_sql(
                base,
                COLUMNS,
                "account".to_owned(),
                None,
                None,
                None,
                CatalogSort::CreatedAt,
                CatalogDirection::Asc,
                Some(cursor),
                10,
            )
            .is_err()
        );
    }

    for (sort, expected) in [
        (
            CatalogSort::Name,
            CatalogCursorValue::Text("name".to_owned()),
        ),
        (CatalogSort::CreatedAt, CatalogCursorValue::Integer(10)),
        (CatalogSort::UpdatedAt, CatalogCursorValue::Integer(20)),
    ] {
        let encoded = record_catalog_cursor(sort, CatalogDirection::Asc, "name", 10, 20, "id");
        assert_eq!(decode_catalog_cursor(&encoded).unwrap().value, expected);
    }
}
