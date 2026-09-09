use super::*;

#[test]
fn bundle_limits_and_main_type_are_enforced() {
    let limits = BundleLimits {
        max_module_bytes: 3,
        ..BundleLimits::default()
    };
    assert_eq!(
        CanonicalBundle::build("index.js", vec![module("index.js", b"1234")], limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleTooLarge
    );
    let mut main = module("index.js", b"hello");
    main.module_type = ModuleType::Text;
    assert_eq!(
        CanonicalBundle::build("index.js", vec![main], BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}
