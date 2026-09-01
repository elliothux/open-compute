//! Subprocess CLI shape tests for `ocd`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use tempfile::TempDir;

#[test]
fn help_subcommands() {
    let bin = env!("CARGO_BIN_EXE_ocd");
    for args in [
        vec!["--help"],
        vec!["config", "check", "--help"],
        vec!["doctor", "--help"],
        vec!["config", "init", "--help"],
        vec!["docs", "--help"],
        vec!["licenses", "--help"],
        vec!["worker", "bundle", "--help"],
    ] {
        let out = Command::new(bin).args(&args).output().expect("run");
        assert!(out.status.success(), "{args:?} {:?}", out.status);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("Usage"));
    }
}

#[test]
fn worker_bundle_reads_stdin_without_loading_platform_configuration() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ocd"))
        .args(["worker", "bundle"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{
        "schemaVersion": 1,
        "mainModule": "worker.js",
        "modules": [{"name":"worker.js","type":"esModule","bytesBase64":"ZXhwb3J0IGRlZmF1bHQge307"}]
    }"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle = open_compute_workers::CanonicalBundle::parse(
        output.stdout,
        open_compute_workers::BundleLimits::default(),
    )
    .unwrap();
    assert_eq!(bundle.manifest().main_module, "worker.js");
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_and_relative_config_exit_codes_do_not_echo_secrets() {
    let bin = env!("CARGO_BIN_EXE_ocd");
    let rel = Command::new(bin)
        .args(["config", "check", "--config", "relative.toml"])
        .output()
        .unwrap();
    assert_eq!(rel.status.code(), Some(3));
    let err = String::from_utf8_lossy(&rel.stderr);
    assert!(err.contains("CONFIG_PATH_INVALID"));
    assert!(!err.contains("AKIA"));
    assert!(!String::from_utf8_lossy(&rel.stdout).contains("AKIA"));

    let missing = Command::new(bin)
        .args([
            "config",
            "check",
            "--config",
            "/tmp/open-compute-missing-config.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(3));
}

#[test]
fn config_check_json_is_deterministic_and_secret_free() {
    let bin = env!("CARGO_BIN_EXE_ocd");
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let mut perms = fs::metadata(&data).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&data, perms).unwrap();
    let cfg = dir.path().join("config.toml");
    fs::write(
        &cfg,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:8787"
[storage]
data_dir = "{data}"
master_key_file = "{key}"
[s3]
endpoint = "http://127.0.0.1:9"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
[runtime]
"#,
            data = data.display(),
            key = dir.path().join("master.key").display(),
        ),
    )
    .unwrap();
    let a = Command::new(bin)
        .args([
            "config",
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    let b = Command::new(bin)
        .args([
            "config",
            "check",
            "--config",
            cfg.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(a.status.success());
    assert_eq!(a.stdout, b.stdout);
    let text = String::from_utf8_lossy(&a.stdout);
    assert!(text.contains("\"schema_version\":1") || text.contains("\"schema_version\": 1"));
    assert!(!text.contains("AKIA"));
    assert!(!String::from_utf8_lossy(&a.stderr).contains("wJalr"));
}
