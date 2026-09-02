use crate::{
    DistanceMetric, ExactCandidate, ExactTopK, FilterOperator, MAX_METADATA_BYTES,
    MAX_METADATA_PREDICATES, PreparedQuery, SearchError, compile_filter, decode_f32le,
    encode_f32le, exact_top_k, normalize_public_score, raw_score, validate_metadata,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::str::FromStr;

#[test]
fn stable_error_messages_and_metric_tokens_cover_the_closed_contract() {
    for (error, message) in [
        (
            SearchError::DimensionMismatch,
            "vector dimensions do not match the index",
        ),
        (
            SearchError::NonFiniteVector,
            "vector contains a non-finite component",
        ),
        (
            SearchError::InvalidVectorEncoding,
            "persisted vector encoding is invalid",
        ),
        (
            SearchError::InvalidMetric,
            "vector distance metric is invalid",
        ),
        (SearchError::InvalidIdentity, "vector identity is invalid"),
        (SearchError::InvalidMetadata, "vector metadata is invalid"),
        (
            SearchError::InvalidFilter,
            "vector metadata filter is invalid",
        ),
        (SearchError::InvalidTopK, "vector query topK is invalid"),
    ] {
        assert_eq!(error.to_string(), message);
    }
    for (metric, token) in [
        (DistanceMetric::Cosine, "cosine"),
        (DistanceMetric::Euclidean, "euclidean"),
        (DistanceMetric::DotProduct, "dot-product"),
    ] {
        assert_eq!(metric.as_str(), token);
        assert_eq!(DistanceMetric::from_str(token), Ok(metric));
    }
    assert_eq!(
        DistanceMetric::from_str("angular"),
        Err(SearchError::InvalidMetric)
    );
}

#[test]
fn vector_encoding_is_exact_little_endian_and_rejects_corruption() {
    let values = [1.0, -2.5, 0.25];
    let encoded = encode_f32le(&values, 3).unwrap();
    assert_eq!(&encoded[..4], &1.0_f32.to_le_bytes());
    assert_eq!(decode_f32le(&encoded, 3).unwrap(), values);
    assert_eq!(
        decode_f32le(&encoded[..8], 3),
        Err(SearchError::InvalidVectorEncoding)
    );
    assert_eq!(
        encode_f32le(&[f32::NAN], 1),
        Err(SearchError::NonFiniteVector)
    );
    assert_eq!(encode_f32le(&[], 0), Err(SearchError::DimensionMismatch));
    assert_eq!(encode_f32le(&[1.0], 2), Err(SearchError::DimensionMismatch));
    assert_eq!(
        decode_f32le(&1.0_f32.to_le_bytes(), 0),
        Err(SearchError::InvalidVectorEncoding)
    );
}

#[test]
fn metric_scores_use_f64_accumulation_and_public_normalization() {
    assert_eq!(
        raw_score(DistanceMetric::DotProduct, &[1.0, 2.0], &[3.0, 4.0]).unwrap(),
        11.0
    );
    assert_eq!(
        raw_score(DistanceMetric::Euclidean, &[0.0, 0.0], &[3.0, 4.0]).unwrap(),
        5.0
    );
    assert_eq!(normalize_public_score(DistanceMetric::Cosine, 0.0), 0.0);
    assert_eq!(raw_score(DistanceMetric::Cosine, &[0.0], &[1.0]), Ok(-1.0));
    assert_eq!(raw_score(DistanceMetric::Cosine, &[1.0], &[0.0]), Ok(-1.0));
    let prepared = PreparedQuery::new(DistanceMetric::DotProduct, &[2.0, 3.0], 2).unwrap();
    assert_eq!(prepared.dimensions(), 2);
    assert_eq!(prepared.score(&[4.0, 5.0]), Ok(23.0));
    assert_eq!(prepared.score(&[4.0]), Err(SearchError::DimensionMismatch));
    assert_eq!(normalize_public_score(DistanceMetric::Cosine, 1.1), 1.0);
    assert_eq!(normalize_public_score(DistanceMetric::Cosine, -1.1), -1.0);
}

#[test]
fn metric_specific_ordering_matches_public_distance_contract() {
    let vectors = [
        ("same", [0.0, 0.0]),
        ("near-a", [1.0, 0.0]),
        ("near-b", [0.0, 1.0]),
        ("far", [3.0, 4.0]),
    ];
    let euclidean = exact_top_k(
        DistanceMetric::Euclidean,
        &[0.0, 0.0],
        vectors
            .iter()
            .map(|(id, values)| ExactCandidate { id, values }),
        3,
    )
    .unwrap();
    assert_eq!(euclidean[0].id, "same");
    assert_eq!(euclidean[0].score, 0.0);
    assert_eq!(
        [euclidean[1].id.as_str(), euclidean[2].id.as_str()],
        ["near-a", "near-b"]
    );

    let dots = [("large", [3.0]), ("small", [1.0]), ("negative", [-1.0])];
    let dot = exact_top_k(
        DistanceMetric::DotProduct,
        &[2.0],
        dots.iter()
            .map(|(id, values)| ExactCandidate { id, values }),
        2,
    )
    .unwrap();
    assert_eq!(
        dot.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["large", "small"]
    );
    assert_eq!(dot[0].score, 6.0);

    let cosine_zero = exact_top_k(
        DistanceMetric::Cosine,
        &[0.0, 0.0],
        [
            ExactCandidate {
                id: "nonzero",
                values: &[1.0, 0.0],
            },
            ExactCandidate {
                id: "zero",
                values: &[0.0, 0.0],
            },
        ],
        2,
    )
    .unwrap();
    assert_eq!(
        cosine_zero
            .iter()
            .map(|value| (value.id.as_str(), value.score))
            .collect::<Vec<_>>(),
        [("nonzero", -1.0), ("zero", -1.0)]
    );
}

#[test]
fn exact_top_k_is_stable_and_heap_bounded_by_k() {
    let vectors = [
        ("z", [1.0, 0.0]),
        ("a", [1.0, 0.0]),
        ("middle", [0.5, 0.5]),
        ("low", [-1.0, 0.0]),
    ];
    let candidates = vectors
        .iter()
        .map(|(id, values)| ExactCandidate { id, values });
    let matches = exact_top_k(DistanceMetric::Cosine, &[1.0, 0.0], candidates, 3).unwrap();
    assert_eq!(
        matches
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z", "middle"]
    );
    assert_eq!(
        exact_top_k(DistanceMetric::Cosine, &[1.0], [], 0),
        Err(SearchError::InvalidTopK)
    );
    let mut top = ExactTopK::new(DistanceMetric::DotProduct, &[1.0], 1).unwrap();
    top.push(ExactCandidate {
        id: "z",
        values: &[1.0],
    })
    .unwrap();
    top.push(ExactCandidate {
        id: "a",
        values: &[1.0],
    })
    .unwrap();
    top.push(ExactCandidate {
        id: "worse",
        values: &[0.0],
    })
    .unwrap();
    assert_eq!(top.finish()[0].id, "a");
}

#[test]
fn metadata_is_canonical_bounded_and_rejects_unsupported_values() {
    let metadata =
        validate_metadata(&json!({"z": true, "nested": {"name": "doc"}, "tags": ["a", "b"]}))
            .unwrap();
    assert_eq!(
        std::str::from_utf8(metadata.canonical_json()).unwrap(),
        r#"{"nested":{"name":"doc"},"tags":["a","b"],"z":true}"#
    );
    assert!(validate_metadata(&json!({"bad": null})).is_err());
    assert!(validate_metadata(&json!([])).is_err());
    assert!(validate_metadata(&json!({"bad.key": true})).is_err());
    assert!(validate_metadata(&json!({"too": {"deep": {"value": 1}}})).is_err());
    assert!(validate_metadata(&json!({"empty": []})).is_err());
    assert!(validate_metadata(&json!({"mixed": ["a", 1]})).is_err());
    let too_large = "x".repeat(MAX_METADATA_BYTES);
    assert!(validate_metadata(&json!({"value": too_large})).is_err());
}

#[test]
fn filter_validates_indexed_paths_and_matches_before_scoring() {
    let indexed = [
        "category".to_string(),
        "nested.year".to_string(),
        "tags".to_string(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let filter = compile_filter(
        &json!({
            "category": {"$in": ["guide", "manual"]},
            "nested.year": {"$gte": 2025},
            "tags": "rust"
        }),
        &indexed,
    )
    .unwrap();
    let matching = validate_metadata(&json!({
        "category": "guide",
        "nested": {"year": 2026},
        "tags": ["rust", "sqlite"]
    }))
    .unwrap();
    assert!(filter.matches(&matching));
    let missing =
        validate_metadata(&json!({"category": "guide", "nested": {"year": 2026}, "tags": ["go"]}))
            .unwrap();
    assert!(!filter.matches(&missing));
    assert!(compile_filter(&json!({"not_indexed": true}), &indexed).is_err());
    let range = compile_filter(
        &json!({"nested.year": {"$gte": 2020, "$lt": 2030}}),
        &indexed,
    )
    .unwrap();
    assert!(range.matches(&matching));
    assert_eq!(range.len(), 2);
    assert!(!range.is_empty());
    assert_eq!(range.predicates()[0].property_path(), "nested.year");
    assert_eq!(
        range.predicates()[0].operator(),
        FilterOperator::GreaterThanOrEqual
    );
    assert_eq!(range.predicates()[0].operands().len(), 1);
    assert!(compile_filter(&json!({}), &indexed).is_err());
    assert!(compile_filter(&json!({"category": "x".repeat(2_048)}), &indexed).is_err());
    assert!(
        compile_filter(
            &json!({"nested.year": {"$eq": 2026, "$ne": 2025}}),
            &indexed
        )
        .is_err()
    );
    assert!(
        compile_filter(
            &json!({"nested.year": {"$gt": 2020, "$gte": 2021}}),
            &indexed
        )
        .is_err()
    );
}

#[test]
fn filter_operator_matrix_handles_scalars_lists_missing_and_invalid_shapes() {
    let indexed = ["flag", "kind", "name", "score", "tags"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let metadata = validate_metadata(&json!({
        "flag": true,
        "kind": "guide",
        "name": "rust",
        "score": 10,
        "tags": ["rust", "sqlite"]
    }))
    .unwrap();
    for filter in [
        json!({"flag": true}),
        json!({"kind": {"$eq": "guide"}}),
        json!({"name": {"$lt": "z"}}),
        json!({"name": {"$lte": "rust"}}),
        json!({"score": {"$gt": 9}}),
        json!({"score": {"$gte": 10}}),
        json!({"tags": {"$in": ["go", "rust"]}}),
        json!({"tags": {"$nin": ["go", "zig"]}}),
        json!({"missing": null}),
    ] {
        let mut paths = indexed.clone();
        paths.insert("missing".to_string());
        assert!(
            compile_filter(&filter, &paths).unwrap().matches(&metadata),
            "{filter}"
        );
    }
    for filter in [
        json!({"flag": {"$gt": true}}),
        json!({"kind": {"$in": []}}),
        json!({"kind": {"$unknown": "x"}}),
        json!({"kind": {}}),
        json!({"kind": {"$eq": "x", "$gt": "a", "$lt": "z"}}),
        json!({"kind": ["x"]}),
        json!({"$bad.path": "x"}),
        json!({"bad..path": "x"}),
    ] {
        let mut paths = indexed.clone();
        paths.insert("$bad.path".to_string());
        paths.insert("bad..path".to_string());
        assert_eq!(
            compile_filter(&filter, &paths),
            Err(SearchError::InvalidFilter),
            "{filter}"
        );
    }
    assert_eq!(
        compile_filter(&json!({"kind": {"$in": vec!["x"; 101]}}), &indexed,),
        Err(SearchError::InvalidFilter)
    );
    let too_many = (0..=MAX_METADATA_PREDICATES)
        .map(|index| (format!("p{index}"), json!(index)))
        .collect::<serde_json::Map<_, _>>();
    let too_many_paths = too_many.keys().cloned().collect();
    assert_eq!(
        compile_filter(&serde_json::Value::Object(too_many), &too_many_paths),
        Err(SearchError::InvalidFilter)
    );
}

#[test]
fn negative_operators_include_missing_fields() {
    let indexed = ["kind".to_string()].into_iter().collect();
    let ne = compile_filter(&json!({"kind": {"$ne": "secret"}}), &indexed).unwrap();
    let nin = compile_filter(&json!({"kind": {"$nin": ["secret"]}}), &indexed).unwrap();
    let missing = validate_metadata(&json!({"other": true})).unwrap();
    assert!(ne.matches(&missing));
    assert!(nin.matches(&missing));
}
