use super::*;

#[test]
fn secret_string_never_leaks() {
    let secret = SecretString::new("super-secret-credential");
    let debug = format!("{secret:?}");
    let display = secret.to_string();
    let json = serde_json::to_string(&secret).expect("json");
    assert!(!debug.contains("super-secret"));
    assert!(!display.contains("super-secret"));
    assert_eq!(json, "\"[REDACTED]\"");
    assert_eq!(secret.expose(), "super-secret-credential");
}

#[test]
fn secret_bytes_never_leak() {
    let secret = SecretBytes::new(b"master-key-material".to_vec());
    assert!(!format!("{secret:?}").contains("master-key"));
    assert_eq!(secret.to_string(), REDACTED);
    assert_eq!(
        serde_json::to_string(&secret).expect("json"),
        "\"[REDACTED]\""
    );
}

#[test]
fn secret_wrappers_deserialize_without_exposing_values() {
    let text: SecretString = serde_json::from_str(r#""hidden""#).unwrap();
    assert_eq!(text.expose(), "hidden");
}
