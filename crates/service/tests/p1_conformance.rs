//! P1.0 release/capability contract black-box Gate.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace")
        .to_path_buf()
}

fn config(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("platform.toml");
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"

[storage]
data_dir = "{data}"
master_key_file = "{key}"

[s3]
endpoint = "http://127.0.0.1:9"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "P1_TEST_S3_ACCESS_KEY"
secret_access_key_env = "P1_TEST_S3_SECRET_KEY"
prefix = "system/"
r2_prefix = "tenant/r2/"

[runtime]

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
"#,
            data = temp.path().join("data").display(),
            key = temp.path().join("recovery.key").display(),
        ),
    )
    .expect("write config");
    path
}

fn capabilities(config: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_ocd"))
        .args([
            "--config",
            config.to_str().expect("config utf8"),
            "capabilities",
            "--json",
        ])
        .output()
        .expect("run ocd");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("capability json")
}

#[test]
fn p1_capabilities_are_complete_and_identical_across_fresh_processes() {
    let temp = TempDir::new().expect("temp");
    let config = config(&temp);
    let first = capabilities(&config);
    assert_eq!(first["schema_version"], 1);
    assert_eq!(first["release"]["rust_msrv"], "1.98.0");
    assert_eq!(
        first["runtime"]["workerd_lock_sha256"],
        first["release"]["workerd_lock_sha256"]
    );
    let products = first["products"].as_object().expect("products");
    for name in ["deployments", "service_bindings", "workers_cache"] {
        assert_eq!(
            products[name]["status"], "supported_with_deviation",
            "{name}"
        );
        assert_eq!(products[name]["kind"], "platform", "{name}");
        assert_eq!(products[name]["capability_version"], 1, "{name}");
        assert!(products[name].get("methods").is_none(), "{name}");
        assert!(products[name].get("members").is_none(), "{name}");
    }
    for name in [
        "workers",
        "kv",
        "r2",
        "d1",
        "durable_objects",
        "alarms",
        "queues",
        "cron",
        "workflows",
        "cache_api",
        "ai",
        "vectorize",
        "version_metadata",
        "websocket_hibernation",
    ] {
        assert_eq!(products[name]["kind"], "target", "{name}");
        assert!(
            matches!(
                products[name]["status"].as_str(),
                Some("supported" | "supported_with_deviation")
            ),
            "{name}"
        );
        assert_eq!(products[name]["capability_version"], 1, "{name}");
        assert!(products[name].get("methods").is_none(), "{name}");
        assert!(
            products[name]["members"]
                .as_array()
                .expect(name)
                .iter()
                .all(|member| member["status"] != "blocked"),
            "{name}"
        );
    }
    for name in ["static_assets", "images"] {
        assert_eq!(
            products[name]["status"], "supported_with_deviation",
            "{name}"
        );
        assert_eq!(products[name]["kind"], "platform", "{name}");
        assert_eq!(products[name]["capability_version"], 1, "{name}");
        assert!(products[name].get("members").is_none(), "{name}");
    }
    for name in [
        "analytics_engine",
        "browser_rendering",
        "hyperdrive",
        "mtls",
        "rate_limiting",
        "workers_for_platforms",
    ] {
        assert_eq!(products[name]["status"], "unsupported", "{name}");
        assert_eq!(products[name]["kind"], "non_target", "{name}");
        assert!(products[name].get("capability_version").is_none(), "{name}");
        assert!(products[name].get("members").is_none(), "{name}");
    }
    assert!(
        first["runtime"]["workers_types_version"]
            .as_str()
            .expect("workers_types_version")
            .starts_with("5.")
    );
    assert_eq!(
        first["runtime"]["workers_types_ast_sha256"]
            .as_str()
            .expect("ast")
            .len(),
        64
    );
    let deviations = fs::read_to_string(workspace().join("docs/references/p1-deviations.md"))
        .expect("deviations document");
    for product in products.values() {
        if let Some(ids) = product.get("deviations").and_then(Value::as_array) {
            for id in ids {
                assert!(deviations.contains(id.as_str().expect("deviation id")));
            }
        }
    }
    assert_eq!(capabilities(&config), first);
    assert_eq!(
        capabilities(&config)["runtime"]["effective_compatibility_date"],
        "2026-08-30"
    );
    assert!(
        capabilities(&config)["runtime"]
            .get("allowed_flags")
            .is_none()
    );
    assert!(
        capabilities(&config)["runtime"]
            .get("compatibility_date_min")
            .is_none()
    );

    let fixture_root = workspace().join("crates/service/tests/fixtures/p1-conformance");
    for (file, owner) in [
        ("workers.mjs", "checkWorkersSurface"),
        ("kv.mjs", "checkKvSurface"),
        ("r2.mjs", "checkR2Surface"),
        ("d1.mjs", "checkD1Surface"),
        ("durable-objects.mjs", "checkDurableObjectSurface"),
        ("alarms.mjs", "checkAlarmSurface"),
        ("websocket.mjs", "checkWebSocketSurface"),
        ("adversarial-values.mjs", "checkAdversarialValues"),
        ("malicious-worker.mjs", "checkMaliciousWorkerSurface"),
    ] {
        let source = fs::read_to_string(fixture_root.join(file)).expect("conformance fixture");
        assert!(source.contains(owner), "{file} missing {owner}");
    }
}
