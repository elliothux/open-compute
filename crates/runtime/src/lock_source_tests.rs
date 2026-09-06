//! Fork provenance and independent upstream type identity.

use super::*;

#[test]
fn lock_source_provenance_and_independent_types_are_validated() {
    let mut good: serde_json::Value =
        serde_json::from_str(&lock_json(&"ab".repeat(32), "")).unwrap();
    good["workersTypes"]["gitHead"] = serde_json::json!("12".repeat(20));
    good["targets"][host_target()]["archiveUrl"] = serde_json::Value::Null;
    let parsed = RuntimeLock::parse(&serde_json::to_vec(&good).unwrap()).unwrap();
    assert!(parsed.current_target().unwrap().1.archive_url.is_none());
    assert_ne!(parsed.workers_types.git_head, parsed.revision);
    for (pointer, invalid) in [
        (
            "/source/repository",
            serde_json::json!("https://github.com/unrelated/workerd"),
        ),
        ("/source/upstreamBase", serde_json::json!("not-a-revision")),
        ("/source/buildInputs", serde_json::json!({})),
        ("/source/buildInputs/bazel", serde_json::json!("")),
        ("/source/buildInputs/bazel", serde_json::json!("9.2.0\n")),
        (
            "/source/buildInputs/bazel",
            serde_json::json!("x".repeat(2049)),
        ),
        ("/source/buildInputs/mode", serde_json::json!("fastbuild")),
        (
            "/source/buildInputs/target",
            serde_json::json!("//unrelated:server"),
        ),
        ("/release", serde_json::json!("../untrusted")),
        ("/release", serde_json::json!("x".repeat(129))),
    ] {
        let mut value = good.clone();
        *value.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            RuntimeLock::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{pointer}"
        );
    }
    for invalid_key in [String::new(), "x".repeat(129), "control\u{7f}".to_owned()] {
        let mut value = good.clone();
        value["source"]["buildInputs"][invalid_key] = serde_json::json!("value");
        assert!(RuntimeLock::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = good.clone();
    for index in 0..65 {
        value["source"]["buildInputs"][format!("extra-{index}")] = serde_json::json!("value");
    }
    assert!(RuntimeLock::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    good.as_object_mut().unwrap().remove("source");
    assert!(RuntimeLock::parse(&serde_json::to_vec(&good).unwrap()).is_err());
}
