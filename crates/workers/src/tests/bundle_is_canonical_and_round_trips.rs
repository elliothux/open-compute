use super::*;

#[test]
fn bundle_is_canonical_and_round_trips() {
    let a = CanonicalBundle::build(
        "src/index.js",
        vec![
            module("src/z.js", b"export const z = 1;"),
            module("src/index.js", b"export default { fetch() {} };"),
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let b = CanonicalBundle::build(
        "src/index.js",
        vec![
            module("src/index.js", b"export default { fetch() {} };"),
            module("src/z.js", b"export const z = 1;"),
        ],
        BundleLimits::default(),
    )
    .unwrap();
    assert_eq!(a.bytes(), b.bytes());
    assert_eq!(a.sha256(), b.sha256());
    let parsed = CanonicalBundle::parse(a.bytes().to_vec(), BundleLimits::default()).unwrap();
    assert_eq!(parsed.manifest(), a.manifest());
    assert_eq!(parsed.manifest().modules[0].name, "src/index.js");
}
