use super::*;
use serde_json::json;

fn leaf(key: &str, operation: &str, kind: &str, value: Option<Value>) -> FilterNode {
    let mut object = serde_json::Map::from_iter([
        ("key".to_owned(), Value::String(key.to_owned())),
        ("operation".to_owned(), Value::String(operation.to_owned())),
        ("type".to_owned(), Value::String(kind.to_owned())),
    ]);
    if let Some(value) = value {
        object.insert("value".to_owned(), value);
    }
    serde_json::from_value(Value::Object(object)).unwrap()
}

#[test]
fn scalar_filter_operations_validate_and_match() {
    let event = json!({
        "source": {"text":"invoice-paid", "count": 7, "ok": true, "empty": null}
    });
    let cases = [
        ("eq", "string", json!("invoice-paid"), true),
        ("=", "number", json!(7), true),
        ("neq", "boolean", json!(false), true),
        ("!=", "string", json!("other"), true),
        ("includes", "string", json!("paid"), true),
        ("DOES_NOT_INCLUDE", "string", json!("failed"), true),
        ("starts_with", "string", json!("invoice"), true),
        ("ENDS_WITH", "string", json!("paid"), true),
        ("MATCH_REGEX", "string", json!("^invoice-[a-z]+$"), true),
        ("IN", "string", json!("other, invoice-paid"), true),
        ("not_in", "string", json!("other,missing"), true),
        (">", "number", json!(6), true),
        (">=", "number", json!(7), true),
        ("<", "number", json!(8), true),
        ("<=", "number", json!(7), true),
    ];
    for (operation, kind, expected, matches_value) in cases {
        let key = match kind {
            "number" => "source.count",
            "boolean" => "source.ok",
            _ => "source.text",
        };
        let node = leaf(key, operation, kind, Some(expected));
        validate(std::slice::from_ref(&node)).unwrap();
        assert_eq!(
            matches(std::slice::from_ref(&node), Combination::And, &event).unwrap(),
            matches_value,
            "{operation}"
        );
    }

    for (operation, key, expected) in [
        ("EXISTS", "source.text", true),
        ("exists", "source.missing", false),
        ("DOES_NOT_EXIST", "source.empty", true),
        ("is_null", "source.missing", true),
    ] {
        let node = leaf(key, operation, "string", None);
        validate(std::slice::from_ref(&node)).unwrap();
        assert_eq!(
            matches(std::slice::from_ref(&node), Combination::And, &event).unwrap(),
            expected
        );
    }

    let false_node = leaf("source.text", "eq", "string", Some(json!("no")));
    assert!(!matches(std::slice::from_ref(&false_node), Combination::And, &event).unwrap());
    assert!(matches(&[], Combination::And, &event).unwrap());
    assert!(
        matches(
            &[
                false_node,
                leaf("source.ok", "eq", "boolean", Some(json!(true)))
            ],
            Combination::UpperOr,
            &event
        )
        .unwrap()
    );
}

#[test]
fn nested_filters_and_projection_helpers_are_bounded() {
    let filters: Vec<FilterNode> = serde_json::from_value(json!([{
        "kind":"group",
        "filterCombination":"AND",
        "filters":[
            {"key":"source.text","operation":"includes","type":"string","value":"paid"},
            {"key":"source.count","operation":"gte","type":"number","value":7}
        ]
    }]))
    .unwrap();
    validate(&filters).unwrap();
    let event = json!({"source":{"text":"paid", "count":8}});
    assert!(matches(&filters, Combination::UpperAnd, &event).unwrap());
    let mut keys = BTreeSet::new();
    collect_keys(&filters, &mut keys);
    assert_eq!(
        keys,
        BTreeSet::from(["source.count".to_owned(), "source.text".to_owned()])
    );
    assert_eq!(field_value(&event, "source.count"), Some(&json!(8)));
    assert!(field_value(&event, "source.missing").is_none());

    let mut flattened = BTreeMap::new();
    flatten_public(
        "",
        &json!({"a":{"b":1},"array":[1],"null":null}),
        &mut flattened,
        0,
    );
    assert_eq!(flattened["a.b"], 1);
    assert_eq!(flattened["null"], Value::Null);
    assert!(!flattened.contains_key("array"));
    assert_eq!(scalar_kind(&json!("text")), Some("string"));
    assert_eq!(scalar_kind(&json!(1)), Some("number"));
    assert_eq!(scalar_kind(&json!(true)), Some("boolean"));
    assert_eq!(scalar_kind(&Value::Null), None);
}

#[test]
fn invalid_filter_shapes_fail_closed() {
    let invalid = [
        leaf("", "eq", "string", Some(json!("x"))),
        leaf(&"x".repeat(513), "eq", "string", Some(json!("x"))),
        leaf("x", "unknown", "string", Some(json!("x"))),
        leaf("x", "eq", "string", None),
        leaf("x", "exists", "string", Some(json!("x"))),
        leaf("x", "includes", "number", Some(json!(1))),
        leaf("x", "gt", "string", Some(json!("x"))),
        leaf("x", "regex", "string", Some(json!("("))),
        leaf("x", "regex", "string", Some(json!("x".repeat(513)))),
    ];
    for node in invalid {
        assert_eq!(
            validate(&[node]).unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }
    let empty_group: Vec<FilterNode> = serde_json::from_value(json!([{
        "kind":"group", "filterCombination":"and", "filters":[]
    }]))
    .unwrap();
    assert!(validate(&empty_group).is_err());
    let too_many = (0..33)
        .map(|index| leaf(&format!("field{index}"), "exists", "string", None))
        .collect::<Vec<_>>();
    assert!(validate(&too_many).is_err());
    let deep: Vec<FilterNode> = serde_json::from_value(json!([{
        "kind":"group","filterCombination":"and","filters":[{
            "kind":"group","filterCombination":"and","filters":[{
                "kind":"group","filterCombination":"and","filters":[{
                    "kind":"group","filterCombination":"and","filters":[{
                        "key":"x","operation":"exists","type":"string"
                    }]
                }]
            }]
        }]
    }]))
    .unwrap();
    assert!(validate(&deep).is_err());
}
