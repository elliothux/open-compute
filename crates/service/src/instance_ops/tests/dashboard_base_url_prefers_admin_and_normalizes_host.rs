use super::*;

#[test]
fn dashboard_base_url_prefers_admin_and_normalizes_host() {
    let descriptor = GenerationDescriptor {
        schema_version: CONTROL_SCHEMA_VERSION,
        instance_id: "abcde".to_owned(),
        canonical_config_path: "/tmp/c.toml".to_owned(),
        startup_id: "s".to_owned(),
        platform_id: "p".to_owned(),
        release_version: "0.1.0".to_owned(),
        service_scope: ServiceScope::User,
        public_listener: Some("127.0.0.1:1".to_owned()),
        admin_listener: None,
        readiness: "ready".to_owned(),
        published_at: 0,
    };
    assert_eq!(
        dashboard_base_url(&descriptor).unwrap(),
        "http://127.0.0.1:1/operator/"
    );

    let mut with_scheme = descriptor.clone();
    with_scheme.admin_listener = Some("https://127.0.0.1:8443/".to_owned());
    assert_eq!(
        dashboard_base_url(&with_scheme).unwrap(),
        "https://127.0.0.1:8443/operator/"
    );

    let mut missing = descriptor;
    missing.public_listener = None;
    missing.admin_listener = None;
    assert_eq!(
        dashboard_base_url(&missing).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
}
