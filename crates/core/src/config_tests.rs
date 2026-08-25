use super::*;
use crate::redact::Redactor;
use crate::secret::SecretString;

fn parse_ok(toml: &str) -> PlatformConfig {
    PlatformConfig::from_toml_str(toml).unwrap_or_else(|err| panic!("{err}"))
}

fn parse_err(toml: &str) -> PlatformError {
    PlatformConfig::from_toml_str(toml).expect_err("expected config error")
}

#[test]
fn documented_defaults_validate() {
    let config = parse_ok("");
    assert_eq!(config.server.public_bind, "127.0.0.1:8787");
    assert!(config.server.admin_bind.is_none());
    assert!(config.server.trusted_proxies.is_empty());
    assert_eq!(
        config.storage.data_dir,
        PathBuf::from("/var/lib/open-compute")
    );
    assert_eq!(config.storage.sqlite_busy_timeout_ms, 5_000);
    assert_eq!(config.s3.prefix, "system/");
    assert_eq!(config.s3.r2_prefix, "tenant/r2/");
    assert_eq!(config.r2.max_object_bytes, 512 * 1024 * 1024);
    assert!(config.s3.verify_tls);
    assert!(config.s3.force_path_style);
    assert_eq!(
        config.runtime.lock_file,
        PathBuf::from("/opt/open-compute/runtime/workerd.lock.json")
    );
    assert_eq!(
        config.runtime.assets_dir,
        PathBuf::from("/opt/open-compute/runtime")
    );
    assert_eq!(
        config.storage.data_lock_path(),
        PathBuf::from("/var/lib/open-compute/platform.lock")
    );
    assert_eq!(config.runtime.startup_timeout_ms, 20_000);
    assert_eq!(config.runtime.shutdown_grace_ms, 10_000);
    assert_eq!(config.cache.max_bytes, 10_737_418_240);
    assert!((config.cache.low_watermark_ratio - 0.80).abs() < f64::EPSILON);
    assert!((config.cache.high_watermark_ratio - 0.90).abs() < f64::EPSILON);
    assert!(config.metrics.enabled);
    assert_eq!(config.diagnostics.max_failed_starts, 32);
    config.validate().expect("defaults");
}

#[test]
fn unknown_fields_are_rejected() {
    let err = parse_err("[server]\nunknown = true\n");
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);
    let err = parse_err("[not_a_table]\nx = 1\n");
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);
}

#[test]
fn example_config_from_design_parses() {
    let toml = r#"
[server]
public_bind = "127.0.0.1:8787"

[storage]
data_dir = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[s3]
endpoint = "https://s3.example.com"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"

[runtime]
binary = "/opt/open-compute/bin/workerd"
startup_timeout_ms = 20000
shutdown_grace_ms = 10000

[cache]
max_bytes = 10737418240
low_watermark_ratio = 0.80
"#;
    let config = parse_ok(toml);
    assert_eq!(config.s3.bucket, "open-compute");
}

#[test]
fn relative_and_parent_paths_are_rejected() {
    let cases = [
        "[storage]\ndata_dir = \"relative/data\"\n",
        "[storage]\nmaster_key_file = \"./master.key\"\n",
        "[runtime]\nbinary = \"workerd\"\n",
        "[runtime]\nlock_file = \"/tmp/../lock\"\n",
        "[runtime]\nassets_dir = \"assets\"\n",
        "[s3]\naccess_key_id_file = \"creds\"\naccess_key_id_env = \"S3_ACCESS_KEY_ID\"\n",
    ];
    for toml in cases {
        let err = parse_err(toml);
        assert_eq!(err.code(), ErrorCode::PathInvalid, "{toml}");
    }
}

#[test]
fn bootstrap_config_path_must_be_absolute() {
    assert!(validate_bootstrap_config_path(Path::new("/etc/open-compute.toml")).is_ok());
    let err = validate_bootstrap_config_path(Path::new("open-compute.toml")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    let err = validate_bootstrap_config_path(Path::new("/etc/../tmp/x.toml")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn admin_non_loopback_requires_auth() {
    let err = parse_err("[server]\nadmin_bind = \"0.0.0.0:8788\"\n");
    assert_eq!(err.code(), ErrorCode::AdminAuthRequired);

    let ok = parse_ok(
        r#"
[server]
admin_bind = "0.0.0.0:8788"
[server.admin_auth]
env = "ADMIN_TOKEN"
"#,
    );
    assert_eq!(ok.server.admin_bind.as_deref(), Some("0.0.0.0:8788"));

    let loopback = parse_ok("[server]\nadmin_bind = \"127.0.0.1:9\"\n");
    assert!(loopback.server.admin_auth.is_none());
}

#[test]
fn secret_refs_must_be_mutually_valid() {
    let err = parse_err("[s3]\naccess_key_id_env = \"\"\n");
    assert_eq!(err.code(), ErrorCode::SecretRefInvalid);
    let err = parse_err("[s3]\naccess_key_id_env = \"lowercase\"\n");
    assert_eq!(err.code(), ErrorCode::SecretRefInvalid);
    let both = parse_ok(
        r#"
[s3]
access_key_id_env = "S3_ACCESS_KEY_ID"
access_key_id_file = "/var/lib/open-compute/keys/s3-access"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
secret_access_key_file = "/var/lib/open-compute/keys/s3-secret"
"#,
    );
    assert!(both.s3.access_key_id_file.is_some());
}

#[test]
fn s3_prefix_and_timeout_bounds() {
    assert_eq!(
        parse_err("[s3]\nprefix = \"system\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nprefix = \"/system/\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nprefix = \"../system/\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nprefix = \"tenant/foo/\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nr2_prefix = \"system/r2/\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nr2_prefix = \"tenant//r2/\"\n").code(),
        ErrorCode::S3PrefixInvalid
    );
    assert_eq!(
        parse_err("[s3]\nmax_retries = 0\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[s3]\nconnect_timeout_ms = 10000\nrequest_timeout_ms = 1000\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[s3]\nendpoint = \"not-a-url\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[s3]\nverify_tls = false\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[s3]\nendpoint = \"https://user:pass@s3.example.com\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[s3]\nendpoint = \"https://s3.example.com/?x=1\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[s3]\nendpoint = \"https://s3.example.com/#frag\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[s3]\nendpoint = \"ftp://s3.example.com\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    let local = parse_ok("[s3]\nendpoint = \"http://127.0.0.1:9000\"\n");
    assert_eq!(local.s3.endpoint, "http://127.0.0.1:9000");
    assert!(local.s3.verify_tls);
}

#[test]
fn r2_bounds_fail_closed() {
    for input in [
        "[r2]\nmax_object_bytes = 0\n",
        "[r2]\nmax_object_bytes = 5363466241\n",
        "[r2]\nmax_concurrent_uploads = 0\n",
        "[r2]\nmax_staging_bytes = 1\n",
        "[r2]\nmax_metadata_head_concurrency = 0\n",
        "[r2]\noperation_timeout_ms = 0\n",
        "[r2]\ncursor_ttl_ms = 86400001\n",
    ] {
        assert_eq!(parse_err(input).code(), ErrorCode::LimitInvalid, "{input}");
    }
}

#[test]
fn trusted_proxies_must_be_cidrs() {
    assert_eq!(
        parse_err("[server]\ntrusted_proxies = [\"\"]\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[server]\ntrusted_proxies = [\"not-a-cidr\"]\n").code(),
        ErrorCode::ConfigInvalid
    );
    let ok = parse_ok(
        r#"
[server]
trusted_proxies = ["10.0.0.0/8", "2001:db8::/32"]
"#,
    );
    assert_eq!(
        ok.server.trusted_proxies,
        vec!["10.0.0.0/8".to_string(), "2001:db8::/32".to_string()]
    );
}

#[test]
fn cache_watermark_and_size_bounds() {
    assert_eq!(
        parse_err("[cache]\nlow_watermark_ratio = 0.95\nhigh_watermark_ratio = 0.90\n").code(),
        ErrorCode::CacheBoundsInvalid
    );
    assert_eq!(
        parse_err("[cache]\nlow_watermark_ratio = 0.0\n").code(),
        ErrorCode::CacheBoundsInvalid
    );
    assert_eq!(
        parse_err("[cache]\nhigh_watermark_ratio = 1.0\n").code(),
        ErrorCode::CacheBoundsInvalid
    );
    assert_eq!(
        parse_err("[cache]\nmax_bytes = 0\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[cache]\nmax_artifact_bytes = 999999999999\n").code(),
        ErrorCode::CacheBoundsInvalid
    );
    assert_eq!(
        parse_err("[cache]\npartial_grace_ms = 0\n").code(),
        ErrorCode::LimitInvalid
    );
}

#[test]
fn runtime_and_storage_timeout_bounds() {
    assert_eq!(
        parse_err("[runtime]\nstartup_timeout_ms = 0\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[runtime]\nrestart_backoff_initial_ms = 40000\nrestart_backoff_max_ms = 30\n")
            .code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[storage]\nfree_space_hard_bytes = 99\nfree_space_soft_bytes = 1\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[metrics]\nmax_series = 0\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[diagnostics]\nmax_failed_starts = 0\n").code(),
        ErrorCode::LimitInvalid
    );
}

#[test]
fn d1_policy_defaults_and_hard_bounds_are_validated() {
    let config = parse_ok("");
    assert_eq!(config.d1, D1Config::default());
    for input in [
        "[d1]\ndatabase_quota_bytes = 1\n",
        "[d1]\nmax_open_databases = 0\n",
        "[d1]\nmax_queued_operations_per_database = 0\n",
        "[d1]\nmax_result_rows = 0\n",
        "[d1]\nmax_result_bytes = 0\n",
        "[d1]\nmax_vm_steps = 1000000001\n",
        "[d1]\nquery_timeout_ms = 300001\n",
        "[d1]\nbatch_timeout_ms = 300001\n",
        "[d1]\nidle_handle_ttl_ms = 86400001\n",
    ] {
        assert_eq!(parse_err(input).code(), ErrorCode::LimitInvalid, "{input}");
    }
}

#[test]
fn durable_object_policy_defaults_and_hard_bounds_are_validated() {
    let config = parse_ok("");
    assert_eq!(config.durable_objects, DurableObjectsConfig::default());
    for input in [
        "[durable_objects]\nmax_namespace_name_bytes = 0\n",
        "[durable_objects]\nmax_object_name_bytes = 1025\n",
        "[durable_objects]\nmax_rpc_request_bytes = 16777217\n",
        "[durable_objects]\nmax_rpc_response_bytes = 0\n",
        "[durable_objects]\nmax_fetch_body_bytes = 67108865\n",
        "[durable_objects]\ndispatch_timeout_ms = 300001\n",
        "[durable_objects]\nmax_in_flight_dispatches = 0\n",
        "[durable_objects]\ndisk_high_watermark_percent = 95\ndisk_stop_writes_percent = 95\n",
        "[durable_objects]\ndisk_stop_writes_percent = 100\n",
        "[durable_objects]\nreconcile_batch = 10001\n",
    ] {
        assert_eq!(parse_err(input).code(), ErrorCode::LimitInvalid, "{input}");
    }
}

#[test]
fn parse_does_not_resolve_secrets_or_search_home() {
    let config = parse_ok(
        r#"
[s3]
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
"#,
    );
    let debug = format!("{config:?}");
    assert!(!debug.contains("AKIA"));
    assert_eq!(
        config.s3.access_key_id_env.as_deref(),
        Some("S3_ACCESS_KEY_ID")
    );
}

#[test]
fn injected_credentials_never_appear_in_debug_display_json_or_redaction() {
    let credential = SecretString::new("AKIAEXAMPLESECRET");
    let master = SecretString::new("ocmk1:dGhpcy1pcy1hLW1hc3Rlci1rZXk");
    let token = SecretString::new("internal-runtime-token-xyz");
    let mut redactor = Redactor::new();
    redactor.register_secret_string(&credential);
    redactor.register_secret_string(&master);
    redactor.register_secret_string(&token);

    let err = PlatformError::new(
        ErrorCode::MasterKeyMismatch,
        "master key fingerprint mismatch",
    );
    let blob = format!(
        "{:?} {} {} {}",
        credential,
        master,
        token,
        serde_json::to_string(&err).unwrap()
    );
    let redacted = redactor.redact(&format!(
        "cred={AKIA} key={KEY} token={TOK} reason=MASTER_KEY_MISMATCH",
        AKIA = credential.expose(),
        KEY = master.expose(),
        TOK = token.expose()
    ));
    assert!(!blob.contains("AKIAEXAMPLESECRET"));
    assert!(!blob.contains("ocmk1:"));
    assert!(!blob.contains("internal-runtime-token-xyz"));
    assert!(!redacted.contains("AKIAEXAMPLESECRET"));
    assert!(!redacted.contains("ocmk1:"));
    assert!(!redacted.contains("internal-runtime-token-xyz"));
    assert!(redacted.contains("MASTER_KEY_MISMATCH"));
    assert_eq!(
        serde_json::to_string(&credential).unwrap(),
        "\"[REDACTED]\""
    );
}

#[test]
fn remaining_authority_and_worker_limit_boundaries_fail_closed() {
    for input in [
        "[server]\nadmin_auth = { env = \"bad-name\" }\n",
        "[storage]\nmaster_key_env = \"9BAD\"\n",
        "[storage]\nsqlite_busy_timeout_ms = 0\n",
        "[s3]\nregion = \"\"\n",
        "[s3]\nbucket = \"\"\n",
        "[runtime]\nrestart_backoff_initial_ms = 0\n",
        "[runtime]\nrestart_backoff_max_ms = 0\n",
        "[workers]\nmax_bundle_bytes = 0\n",
        "[workers]\nmax_request_body_bytes = 0\n",
        "[workers]\ndelete_drain_timeout_ms = 0\n",
        "[workers]\nartifact_gc_grace_ms = 0\n",
        "[workers]\nartifact_gc_interval_ms = 0\n",
        "[workers]\ndelete_recovery_batch = 0\n",
        "[workers]\nretain_ready_deployments = 0\n",
        "[workers]\nretain_rejected_deployments = 0\n",
        "[workers]\ndeployment_min_retention_ms = 0\n",
        "[workers]\nmax_bundle_bytes = 67108865\n",
        "[workers]\ndelete_recovery_batch = 10001\n",
    ] {
        let code = parse_err(input).code();
        assert!(
            matches!(
                code,
                ErrorCode::LimitInvalid | ErrorCode::SecretRefInvalid | ErrorCode::ConfigInvalid
            ),
            "input={input:?} code={code}"
        );
    }
    for endpoint in [
        "ftp://example.com",
        "https://user:pass@example.com",
        "https://example.com?query=1",
        "https://example.com#fragment",
    ] {
        let input = format!("[s3]\nendpoint = {endpoint:?}\n");
        assert_eq!(parse_err(&input).code(), ErrorCode::ConfigInvalid);
    }
}

#[test]
fn private_config_helpers_cover_single_source_boundaries() {
    assert_eq!(
        validate_secret_pair(None, None, "test").unwrap_err().code(),
        ErrorCode::SecretRefInvalid
    );
    assert!(validate_secret_pair(None, Some(Path::new("/tmp/secret")), "test").is_ok());
    assert!(validate_secret_pair(Some("TEST_SECRET"), None, "test").is_ok());
    assert!(
        validate_secret_pair(Some("TEST_SECRET"), Some(Path::new("/tmp/secret")), "test").is_ok()
    );
    assert!(is_loopback("::1".parse().unwrap()));
}
