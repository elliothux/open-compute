use super::*;

pub(super) struct PlatformConfigInput<'a> {
    pub(super) temp: &'a tempfile::TempDir,
    pub(super) name: &'a str,
    pub(super) path: &'a std::path::Path,
    pub(super) master_key: &'a std::path::Path,
    pub(super) mock: &'a MockS3,
}

pub(super) fn write_platform_config(input: &PlatformConfigInput<'_>) -> PathBuf {
    let access_key = input.temp.path().join("p1-s3-access-key");
    let secret_key = input.temp.path().join("p1-s3-secret-key");
    fs::write(&access_key, b"AKIAEXAMPLEKEYID01").unwrap();
    fs::write(&secret_key, b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY").unwrap();
    fs::set_permissions(&access_key, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&secret_key, fs::Permissions::from_mode(0o600)).unwrap();
    let admin_token = input
        .temp
        .path()
        .join(format!("{}-admin.token", input.name));
    let deployer_token = input
        .temp
        .path()
        .join(format!("{}-deployer.token", input.name));
    let read_only_token = input
        .temp
        .path()
        .join(format!("{}-read-only.token", input.name));
    fs::write(&admin_token, b"p0-exit-admin\n").unwrap();
    fs::write(&deployer_token, b"p0-exit-deployer\n").unwrap();
    fs::write(&read_only_token, b"p0-exit-read-only\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&deployer_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&read_only_token, fs::Permissions::from_mode(0o600)).unwrap();
    let path = input.temp.path().join(format!("{}.toml", input.name));
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
path = "{data_dir}"
master_key_file = "{master_key}"

[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "system/"
r2_prefix = "tenant/r2/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000

[runtime]

[cache]
max_bytes = 67108864
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 67108864

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
"#,
            data_dir = input.path.display(),
            master_key = input.master_key.display(),
            endpoint = input.mock.endpoint,
            access_key = access_key.display(),
            secret_key = secret_key.display(),
            admin_token = admin_token.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
        ),
    )
    .unwrap();
    path
}
