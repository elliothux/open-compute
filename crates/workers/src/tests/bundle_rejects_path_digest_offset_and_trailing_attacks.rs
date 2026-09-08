use super::*;

#[test]
fn bundle_rejects_path_digest_offset_and_trailing_attacks() {
    for name in [
        "../index.js",
        "/index.js",
        "a\\b.js",
        "a//b.js",
        "a/./b.js",
        "a/\0b.js",
    ] {
        let err = CanonicalBundle::build(
            name,
            vec![module(name, b"export default {}")],
            BundleLimits::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::BundleInvalid);
    }
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![module("index.js", b"export default {}")],
        BundleLimits::default(),
    )
    .unwrap();
    let mut corrupted = bundle.bytes().to_vec();
    *corrupted.last_mut().unwrap() ^= 1;
    assert_eq!(
        CanonicalBundle::parse(corrupted, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let mut trailing = bundle.bytes().to_vec();
    trailing.push(0);
    assert_eq!(
        CanonicalBundle::parse(trailing, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}
