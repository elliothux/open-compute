use super::*;

#[test]
fn product_cursor_hmacs_are_domain_separated_and_reject_tampering() {
    let key = SecretBytes::new(vec![7_u8; 32]);
    let fingerprint = master_key::fingerprint_for_test(key.expose());
    let crypto = SecretCrypto::new(&key, &fingerprint).unwrap();
    let payload = br#"{"v":1}"#;
    let signature = crypto.sign_r2_cursor(payload);
    assert!(crypto.verify_r2_cursor(payload, &signature));
    assert!(!crypto.verify_r2_cursor(br#"{"v":2}"#, &signature));
    assert_ne!(signature, crypto.sign_kv_cursor(payload));

    let vectorize = crypto.sign_vectorize_cursor(payload);
    let ai_search = crypto.sign_ai_search_cursor(payload);
    assert!(crypto.verify_vectorize_cursor(payload, &vectorize));
    assert!(crypto.verify_ai_search_cursor(payload, &ai_search));
    assert!(!crypto.verify_vectorize_cursor(payload, &ai_search));
    assert!(!crypto.verify_ai_search_cursor(payload, &vectorize));
    assert_ne!(vectorize, signature);
    assert_ne!(ai_search, crypto.sign_kv_cursor(payload));
}
