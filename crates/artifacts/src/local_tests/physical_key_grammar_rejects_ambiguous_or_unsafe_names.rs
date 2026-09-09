use super::*;

#[test]
fn physical_key_grammar_rejects_ambiguous_or_unsafe_names() {
    for value in [
        "", "/a", "a/", "a//b", ".", "..", "a/./b", "a/../b", "a\\b", "a\0b", "a\nb",
    ] {
        assert_eq!(ObjectKey::new(value).unwrap_err(), BackendError::InvalidKey);
    }
    assert_eq!(
        ObjectKey::new(format!("a/{}", "x".repeat(256))).unwrap_err(),
        BackendError::InvalidKey
    );
    assert!(ObjectKey::new("system/a.b-c_d=1+2@x").is_ok());
}
