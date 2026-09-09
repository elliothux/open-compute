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
    let admin_token = input.root.join(format!("{}-admin.token", input.name));
    let deployer_token = input.root.join(format!("{}-deployer.token", input.name));
    let read_only_token = input.root.join(format!("{}-read-only.token", input.name));
    write_mode(&admin_token, b"p1-snapshot-admin\n", 0o600);
    write_mode(&deployer_token, b"p1-snapshot-deployer\n", 0o600);
    write_mode(&read_only_token, b"p1-snapshot-read-only\n", 0o600);
    let path = input.root.join(format!("{}.toml", input.name));
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"

[server.admin_auth]
file = "{admin_token}"

[server.deployer_auth]
file = "{deployer_token}"

[server.read_only_auth]
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
max_series = 1024
"#,
            data = input.path.display(),
            key = input.master_key.display(),
            endpoint = input.endpoint,
            access_key = input.access_key.display(),
            secret_key = input.secret_key.display(),
            admin_token = admin_token.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
            prefix = input.prefix,
        ),
    )
    .expect("config");
    path
}

pub(super) async fn run_cli_json(config: &Path, args: &[&str]) -> serde_json::Value {
    let mut argv = vec![
        "ocd".to_owned(),
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|value| (*value).to_owned()));
    let cli = parse_from(argv).expect("parse CLI fixture");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = execute(cli, &mut stdout, &mut stderr).await;
    assert_eq!(
        status,
        std::process::ExitCode::SUCCESS,
        "CLI stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    serde_json::from_slice(&stdout).expect("CLI JSON")
}

pub(super) async fn run_cli_human(config: &Path, args: &[&str]) -> String {
    let mut argv = vec![
        "ocd".to_owned(),
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|value| (*value).to_owned()));
    let cli = parse_from(argv).expect("parse CLI fixture");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = execute(cli, &mut stdout, &mut stderr).await;
    assert_eq!(
        status,
        std::process::ExitCode::SUCCESS,
        "CLI stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    String::from_utf8(stdout).expect("CLI UTF-8")
}
