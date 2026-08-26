//! P1.0 release/capability contract black-box Gate.

use open_compute_workers::{
    COMPATIBILITY_DATE_MAX, COMPATIBILITY_DATE_MIN, COMPATIBILITY_FLAGS_ALLOWED,
    validate_compatibility,
};
use serde_json::Value;
use std::collections::BTreeMap;
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
    let root = workspace();
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
binary = "{binary}"
lock_file = "{lock}"
assets_dir = "{assets}"

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 512
"#,
            data = temp.path().join("data").display(),
            key = temp.path().join("recovery.key").display(),
            binary = temp.path().join("workerd").display(),
            lock = root.join("runtime/workerd.lock.json").display(),
            assets = root.join("runtime").display(),
        ),
    )
    .expect("write config");
    path
}

fn capabilities(config: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_platformd"))
        .args([
            "--config",
            config.to_str().expect("config utf8"),
            "capabilities",
            "--json",
        ])
        .output()
        .expect("run platformd");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("capability json")
}

#[test]
fn p1_capabilities_are_complete_and_identical_across_three_fresh_processes() {
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
    for name in ["workers", "kv", "r2", "d1", "durable_objects", "alarms"] {
        assert_eq!(products[name]["status"], "supported", "{name}");
        assert!(products[name]["capability_version"].is_number(), "{name}");
    }
    for name in ["queues", "cron", "workflows", "websocket_hibernation"] {
        assert_eq!(products[name]["status"], "unsupported", "{name}");
        assert!(products[name].get("capability_version").is_none(), "{name}");
    }
    let expected_methods = BTreeMap::from([
        (
            "workers",
            vec!["fetch", "rpc", "streams", "websocket", "outbound_fetch"],
        ),
        (
            "kv",
            vec!["get", "getWithMetadata", "put", "delete", "list", "getBulk"],
        ),
        (
            "r2",
            vec!["head", "get", "put", "delete", "list", "deleteMany"],
        ),
        (
            "d1",
            vec![
                "prepare",
                "batch",
                "exec",
                "withSession",
                "run",
                "all",
                "first",
                "raw",
            ],
        ),
        (
            "durable_objects",
            vec![
                "idFromName",
                "newUniqueId",
                "idFromString",
                "get",
                "getByName",
                "fetch",
                "rpc",
            ],
        ),
        (
            "alarms",
            vec!["getAlarm", "setAlarm", "deleteAlarm", "alarm"],
        ),
    ]);
    for (product, methods) in expected_methods {
        assert_eq!(
            products[product]["methods"],
            serde_json::json!(methods),
            "{product}"
        );
    }
    assert_eq!(products["durable_objects"]["basic_websocket"], "supported");
    assert_eq!(
        products["durable_objects"]["hibernatable_websocket"],
        "unsupported"
    );
    let deviations =
        fs::read_to_string(workspace().join("docs/p1-deviations.md")).expect("deviations document");
    for product in products.values() {
        if let Some(ids) = product.get("deviations").and_then(Value::as_array) {
            for id in ids {
                assert!(deviations.contains(id.as_str().expect("deviation id")));
            }
        }
    }
    assert_eq!(capabilities(&config), first);
    assert_eq!(capabilities(&config), first);

    for date in [COMPATIBILITY_DATE_MIN, COMPATIBILITY_DATE_MAX] {
        assert_eq!(
            validate_compatibility(
                date,
                COMPATIBILITY_FLAGS_ALLOWED
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            )
            .expect("supported compatibility boundary"),
            ["nodejs_compat".to_owned(), "rpc".to_owned()]
        );
    }
    assert!(validate_compatibility("2021-12-31", Vec::new()).is_err());
    assert!(validate_compatibility("2026-08-24", Vec::new()).is_err());
    assert!(validate_compatibility(COMPATIBILITY_DATE_MIN, vec!["unknown".to_owned()]).is_err());
    assert_eq!(
        validate_compatibility(
            COMPATIBILITY_DATE_MIN,
            vec!["rpc".to_owned(), "rpc".to_owned()],
        )
        .expect("duplicate flag is canonicalized"),
        ["rpc".to_owned()]
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
