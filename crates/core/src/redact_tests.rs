use super::*;

#[test]
fn redacts_registered_secrets_and_preserves_reason_codes() {
    let mut redactor = Redactor::new();
    redactor.register_str("injected-credential");
    assert_eq!(
        redactor.redact("token=injected-credential code=MASTER_KEY_MISMATCH"),
        "token=[REDACTED] code=MASTER_KEY_MISMATCH"
    );
    assert_eq!(
        redactor.redact("MASTER_KEY_MISMATCH"),
        "MASTER_KEY_MISMATCH"
    );
    assert_eq!(
        redactor.redact_bytes(b"injected-credential"),
        b"[REDACTED]".to_vec()
    );
}

#[test]
fn registered_reason_like_secrets_are_always_redacted() {
    let mut redactor = Redactor::new();
    redactor.register_str("READY");
    redactor.register_str("MASTER_KEY_MISMATCH");
    assert_eq!(redactor.redact("READY"), "[REDACTED]");
    assert_eq!(redactor.redact("MASTER_KEY_MISMATCH"), "[REDACTED]");
    assert_eq!(
        redactor.redact("status=READY code=MASTER_KEY_MISMATCH"),
        "status=[REDACTED] code=[REDACTED]"
    );
    assert_eq!(redactor.redact_bytes(b"READY"), b"[REDACTED]".to_vec());

    let untouched = Redactor::new();
    assert_eq!(untouched.redact("READY"), "READY");
    assert_eq!(
        untouched.redact("MASTER_KEY_MISMATCH"),
        "MASTER_KEY_MISMATCH"
    );
}

#[test]
fn empty_and_typed_secrets_are_handled_without_duplicate_registration() {
    let mut redactor = Redactor::new();
    redactor.register_str("");
    redactor.register_bytes(b"");
    let text = SecretString::new("long-secret");
    let bytes = SecretBytes::new(b"secret".to_vec());
    redactor.register_secret_string(&text);
    redactor.register_secret_bytes(&bytes);
    redactor.register_bytes(b"secret");
    assert_eq!(
        redactor.redact("long-secret secret"),
        "[REDACTED] [REDACTED]"
    );
    assert_eq!(redactor.redact_bytes(b""), Vec::<u8>::new());
}
