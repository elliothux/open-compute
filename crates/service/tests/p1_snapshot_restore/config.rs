use super::*;

pub(super) fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

pub(super) struct ConfigInputs<'a> {
    pub(super) root: &'a Path,
    pub(super) name: &'a str,
    pub(super) path: &'a Path,
    pub(super) master_key: &'a Path,
    pub(super) access_key: &'a Path,
    pub(super) secret_key: &'a Path,
    pub(super) endpoint: &'a str,
    pub(super) prefix: &'a str,
}

pub(super) fn write_config(input: &ConfigInputs<'_>) -> PathBuf {
    let deployer_token = input.root.join(format!("{}-deployer.token", input.name));
    let read_only_token = input.root.join(format!("{}-read-only.token", input.name));
    write_mode(&deployer_token, b"p1-snapshot-deployer\n", 0o600);
    write_mode(&read_only_token, b"p1-snapshot-read-only\n", 0o600);
    let path = input.root.join(format!("{}.toml", input.name));
    fs::write(
        &path,
        format!(
            r#"
[auth]
[auth.deployer_auth]
file = "{deployer_token}"

[auth.read_only_auth]
file = "{read_only_token}"

[data]
path = "{data}"
master_key_file = "{key}"

[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "{prefix}"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000

[runtime]

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536

[metrics]
enabled = true
max_label_value_bytes = 64
"#,
            data = input.path.display(),
            key = input.master_key.display(),
            endpoint = input.endpoint,
            access_key = input.access_key.display(),
            secret_key = input.secret_key.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
            prefix = input.prefix,
        ),
    )
    .expect("config");
    path
}

pub(super) async fn run_cli_json(
    scope_root: &Path,
    config: &Path,
    args: &[&str],
) -> serde_json::Value {
    let stdout = run_cli(scope_root, config, args).await;
    serde_json::from_slice(&stdout).expect("CLI JSON")
}

pub(super) async fn run_cli_human(scope_root: &Path, config: &Path, args: &[&str]) -> String {
    String::from_utf8(run_cli(scope_root, config, args).await).expect("CLI UTF-8")
}

async fn run_cli(scope_root: &Path, config: &Path, args: &[&str]) -> Vec<u8> {
    let output = run_cli_output(scope_root, config, args).await;
    assert!(
        output.status.success(),
        "CLI {:?} stderr: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    output.stdout
}

pub(super) async fn run_cli_error(scope_root: &Path, config: &Path, args: &[&str]) -> String {
    let output = run_cli_output(scope_root, config, args).await;
    assert!(!output.status.success(), "CLI unexpectedly succeeded");
    String::from_utf8(output.stderr).expect("CLI stderr UTF-8")
}

async fn run_cli_output(scope_root: &Path, config: &Path, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_ocd"))
        .arg("--no-update-check")
        .arg("--config")
        .arg(config)
        .args(args)
        .env("OPEN_COMPUTE_TEST_OCD_ROOT", scope_root)
        .env_remove("S3_ACCESS_KEY_ID")
        .env_remove("S3_SECRET_ACCESS_KEY")
        .output()
        .await
        .expect("run CLI fixture")
}
