use super::*;

#[test]
fn lock_validation_rejects_every_malformed_authority_field() {
    let good = lock_json(&"ab".repeat(32), "");
    let replacements = [
        ("\"release\": \"v1.20260830.1\"", "\"release\": \"\""),
        (
            "\"release\": \"v1.20260830.1\"",
            "\"release\": \" v1.20260830.1\"",
        ),
        (
            "\"revision\": \"e9dda5963aba7ee4323960db795690ec78fec118\"",
            "\"revision\": \"\"",
        ),
        (
            "\"revision\": \"e9dda5963aba7ee4323960db795690ec78fec118\"",
            "\"revision\": \"E9DDA5963ABA7EE4323960DB795690EC78FEC118\"",
        ),
        (
            "\"expectedVersionOutput\": \"workerd 2026-08-30\"",
            "\"expectedVersionOutput\": \"\"",
        ),
        (
            "\"expectedVersionOutput\": \"workerd 2026-08-30\"",
            "\"expectedVersionOutput\": \"workerd 2026-08-30 \"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"20260830\"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"1969-01-01\"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"2026-00-01\"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"2026-13-01\"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"2026-01-00\"",
        ),
        (
            "\"effectiveCompatibilityDate\": \"2026-09-08\"",
            "\"effectiveCompatibilityDate\": \"2100-02-29\"",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": []",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"-x\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--x=y\"]",
        ),
        (
            "\"processFlags\": [\"--experimental\"]",
            "\"processFlags\": [\"--x y\"]",
        ),
        (
            "\"version\": \"314.0.6_2026-08-17_2\"",
            "\"version\": \"../bundle\"",
        ),
        (
            "\"fileName\": \"pyodide_314.0.6_2026-08-17_2.capnp.bin\"",
            "\"fileName\": \"other.capnp.bin\"",
        ),
        (
            "\"archiveName\": \"pyodide_314.0.6_2026-08-17_2.capnp.bin.gz\"",
            "\"archiveName\": \"other.gz\"",
        ),
        (
            "\"bundleSha256\": \"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\"",
            "\"bundleSha256\": \"invalid\"",
        ),
        (
            "\"systemCompatibilityFlags\": [\"experimental\", \"service_binding_extra_handlers\"]",
            "\"systemCompatibilityFlags\": [\"\"]",
        ),
        (
            "\"systemCompatibilityFlags\": [\"experimental\", \"service_binding_extra_handlers\"]",
            "\"systemCompatibilityFlags\": [\"node-js\"]",
        ),
        (
            "\"systemCompatibilityFlags\": [\"experimental\", \"service_binding_extra_handlers\"]",
            "\"systemCompatibilityFlags\": [\"experimental\", \"experimental\"]",
        ),
        (
            "\"requiredCompatibilityFlags\": []",
            "\"requiredCompatibilityFlags\": [\"experimental\"]",
        ),
        (
            "\"workersTypes\": {\n    \"version\": \"5.20260830.1\",\n    \"gitHead\": \"e9dda5963aba7ee4323960db795690ec78fec118\",\n    \"packageSha256\": \"d3d7a80d3b27e53116e34736ec1945eb359f53a1000df37b205c4cb59ce29a8e\",\n    \"astSha256\": \"a00b4783854c9028158f776d605790d9a3e17e6a97f4d255beb70035c59c40dd\"\n  }",
            "\"workersTypes\": {\n    \"version\": \"5.20260830.1\",\n    \"gitHead\": \"invalid-upstream-revision\",\n    \"packageSha256\": \"d3d7a80d3b27e53116e34736ec1945eb359f53a1000df37b205c4cb59ce29a8e\",\n    \"astSha256\": \"a00b4783854c9028158f776d605790d9a3e17e6a97f4d255beb70035c59c40dd\"\n  }",
        ),
        (
            "\"wranglerVersion\": \"4.127.1\"",
            "\"wranglerVersion\": \"\"",
        ),
        (
            "\"vitePluginVersion\": \"1.54.2\"",
            "\"vitePluginVersion\": \" 1.54.2\"",
        ),
    ];
    for (needle, replacement) in replacements {
        let bad = good.replacen(needle, replacement, 1);
        assert_ne!(bad, good, "test replacement must match: {needle}");
        assert_eq!(
            RuntimeLock::parse(bad.as_bytes()).unwrap_err().code(),
            ErrorCode::RuntimeInvalid,
            "malformed lock unexpectedly accepted: {replacement}"
        );
    }

    let mut value: serde_json::Value = serde_json::from_str(&good).unwrap();
    value["targets"] = serde_json::json!({});
    assert!(RuntimeLock::parse(&serde_json::to_vec(&value).unwrap()).is_err());

    for scalar in ["true", "-1", "1", "1.5", "\"lock\"", "null", "[]"] {
        assert!(RuntimeLock::parse(scalar.as_bytes()).is_err());
    }
    assert!(RuntimeLock::parse(&vec![b' '; 1024 * 1024 + 1]).is_err());
}
