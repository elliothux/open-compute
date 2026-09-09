use super::*;

#[test]
fn production_client_rejects_insecure_or_zero_limit_configuration() {
    let creds = resolve_s3_credentials_with(&s3_config("https://s3.example.invalid"), &env())
        .expect("credentials");
    let mut insecure = s3_config("https://s3.example.invalid");
    insecure.verify_tls = false;
    assert_eq!(
        ObjectBackend::connect_s3(&insecure, &creds, 1024)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let secure = s3_config("https://s3.example.invalid");
    assert_eq!(
        ObjectBackend::connect_s3(&secure, &creds, 0)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}
