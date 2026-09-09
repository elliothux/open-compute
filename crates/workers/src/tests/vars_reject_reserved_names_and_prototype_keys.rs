use super::*;

#[test]
fn vars_reject_reserved_names_and_prototype_keys() {
    let mut vars = BTreeMap::new();
    vars.insert("OPEN_COMPUTE_TOKEN".to_owned(), serde_json::json!(1));
    assert!(canonicalize_vars(vars).is_err());
    let mut vars = BTreeMap::new();
    vars.insert(
        "SAFE".to_owned(),
        serde_json::json!({"__proto__": {"x": 1}}),
    );
    assert!(canonicalize_vars(vars).is_err());
}
