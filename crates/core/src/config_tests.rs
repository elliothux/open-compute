use super::*;
use crate::SchedulerKind;
use crate::redact::Redactor;
use crate::secret::SecretString;

fn parse_ok(toml: &str) -> PlatformConfig {
    PlatformConfig::from_toml_str(&complete_config(toml)).unwrap_or_else(|err| panic!("{err}"))
}

fn parse_err(toml: &str) -> PlatformError {
    PlatformConfig::from_toml_str(&complete_config(toml)).expect_err("expected config error")
}

fn complete_config(toml: &str) -> String {
    let mut source = String::new();
    if !toml.contains("[data]") {
        source.push_str(
            "[data]\npath = \"/var/lib/open-compute\"\nmaster_key_file = \"/var/lib/open-compute/keys/master.key\"\n\n",
        );
    }
    if !toml.contains("[storage]") {
        source.push_str(
            "[storage]\nbackend = \"local\"\npath = \"/var/lib/open-compute/objects\"\n\n",
        );
    }
    source.push_str(toml);
    source
}

fn s3(config: &PlatformConfig) -> &S3Config {
    config.object_storage.as_s3().expect("S3 config")
}

#[test]
fn documented_defaults_validate() {
    let config = parse_ok("");
    assert_eq!(config.server.public_bind, "127.0.0.1:8787");
    assert!(config.server.admin_bind.is_none());
    assert_eq!(config.data.path, PathBuf::from("/var/lib/open-compute"));
    assert_eq!(config.data.sqlite_busy_timeout_ms, 5_000);
    assert_eq!(config.object_storage.prefix(), "system/");
    assert_eq!(config.object_storage.r2_prefix(), "tenant/r2/");
    assert_eq!(config.r2.max_object_bytes, 512 * 1024 * 1024);
    assert_eq!(config.object_storage.kind(), ObjectStorageKind::Local);
    assert_eq!(
        config.data.data_lock_path(),
        PathBuf::from("/var/lib/open-compute/platform.lock")
    );
    assert_eq!(config.runtime.startup_timeout_ms, 20_000);
    assert_eq!(config.runtime.shutdown_grace_ms, 10_000);
    assert_eq!(config.cache.max_bytes, 10_737_418_240);
    assert!((config.cache.low_watermark_ratio - 0.80).abs() < f64::EPSILON);
    assert!((config.cache.high_watermark_ratio - 0.90).abs() < f64::EPSILON);
    assert!(config.metrics.enabled);
    config.validate().expect("defaults");
}

#[test]
fn checked_in_default_config_matches_the_current_schema() {
    let source = include_str!("../../../share/default-config.toml");
    let config: PlatformConfig = toml::from_str(source).expect("checked-in default config");
    config
        .validate()
        .expect("checked-in default config validates");
}

#[test]
fn unknown_fields_are_rejected() {
    let err = parse_err("[server]\nunknown = true\n");
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);
    let err = parse_err("[not_a_table]\nx = 1\n");
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);
}

#[test]
fn data_and_object_storage_sections_are_required() {
    assert!(toml::from_str::<PlatformConfig>("").is_err());
    assert!(
        toml::from_str::<PlatformConfig>(
            "[data]\npath = \"/var/lib/open-compute\"\nmaster_key_file = \"/var/lib/open-compute/keys/master.key\"\n"
        )
        .is_err()
    );
    assert!(
        toml::from_str::<PlatformConfig>(
            "[storage]\nbackend = \"local\"\npath = \"/var/lib/open-compute/objects\"\n"
        )
        .is_err()
    );
}

#[test]
fn example_config_from_design_parses() {
    let toml = r#"
[server]
public_bind = "127.0.0.1:8787"

[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "https://s3.example.com"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"

[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 10000

[cache]
max_bytes = 10737418240
low_watermark_ratio = 0.80
"#;
    let config = parse_ok(toml);
    assert_eq!(s3(&config).bucket, "open-compute");
}

#[test]
fn relative_and_parent_paths_are_rejected() {
    let cases = [
        "[data]\npath = \"relative/data\"\n",
        "[data]\nmaster_key_file = \"./master.key\"\n",
        "[storage]\nbackend = \"s3\"\naccess_key_id_file = \"creds\"\naccess_key_id_env = \"S3_ACCESS_KEY_ID\"\n",
    ];
    for toml in cases {
        let err = parse_err(toml);
        assert_eq!(err.code(), ErrorCode::PathInvalid, "{toml}");
    }
}

#[test]
fn config_relative_host_paths_resolve_once_without_shell_expansion() {
    let base = Path::new("/srv/open-compute/config/nested");
    let local = PlatformConfig::from_toml_str_at(
        r#"
[server]
admin_auth = { file = "./secrets/admin" }

[data]
path = "../../state"
master_key_file = "../../state/keys/master.key"

[storage]
backend = "local"
path = "../../state/objects"

[ai.providers.example]
base_url = "http://127.0.0.1:8123/v1"
auth = { kind = "bearer", secret = { file = "~/literal-$HOME-*.key" } }
"#,
        base,
    )
    .unwrap();
    assert_eq!(local.data.path, Path::new("/srv/open-compute/state"));
    assert_eq!(
        local.data.master_key_file,
        Path::new("/srv/open-compute/state/keys/master.key")
    );
    assert_eq!(
        local.server.admin_auth.file.as_deref(),
        Some(Path::new("/srv/open-compute/config/nested/secrets/admin"))
    );
    assert_eq!(
        local.object_storage.as_local().unwrap().path,
        Path::new("/srv/open-compute/state/objects")
    );
    let AiAuthConfig::Bearer { secret } = &local.ai.providers["example"].auth else {
        panic!("expected bearer auth");
    };
    assert_eq!(
        secret.file.as_deref(),
        Some(Path::new(
            "/srv/open-compute/config/nested/~/literal-$HOME-*.key"
        ))
    );

    let s3 = PlatformConfig::from_toml_str_at(
        r#"
[data]
path = "../../state"
master_key_file = "../../state/keys/master.key"

[storage]
backend = "s3"
access_key_id_file = "../credentials/access"
secret_access_key_file = "../credentials/secret"
"#,
        base,
    )
    .unwrap();
    assert_eq!(
        s3.object_storage
            .as_s3()
            .unwrap()
            .access_key_id_file
            .as_deref(),
        Some(Path::new("/srv/open-compute/config/credentials/access"))
    );
    assert_eq!(
        s3.object_storage
            .as_s3()
            .unwrap()
            .secret_access_key_file
            .as_deref(),
        Some(Path::new("/srv/open-compute/config/credentials/secret"))
    );
}

#[test]
fn bootstrap_config_path_accepts_exact_relative_or_absolute_input() {
    assert!(validate_bootstrap_config_path(Path::new("/etc/open-compute.toml")).is_ok());
    assert!(validate_bootstrap_config_path(Path::new("open-compute.toml")).is_ok());
    assert!(validate_bootstrap_config_path(Path::new("/etc/../tmp/x.toml")).is_ok());
    assert_eq!(
        validate_bootstrap_config_path(Path::new(""))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
}

#[test]
fn external_runtime_configuration_is_not_supported() {
    for field in ["binary", "lock_file", "assets_dir"] {
        for value in ["/opt/external-runtime", "relative"] {
            let error = parse_err(&format!("[runtime]\n{field} = {value:?}\n"));
            assert_eq!(error.code(), ErrorCode::ConfigParseFailed);
        }
    }
}

#[test]
fn admin_auth_is_required_for_loopback_and_non_loopback_bind() {
    for bind in ["127.0.0.1:8788", "0.0.0.0:8788"] {
        let err = parse_err(&format!(
            r#"
[server]
public_bind = "127.0.0.1:8787"
admin_bind = "{bind}"

[server.admin_auth]
"#
        ));
        assert_eq!(
            err.code(),
            ErrorCode::SecretRefInvalid,
            "missing admin_auth must fail closed for bind={bind}"
        );
    }

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
    assert_eq!(
        loopback.server.admin_auth.env.as_deref(),
        Some("OPEN_COMPUTE_ADMIN_TOKEN")
    );
    assert_eq!(
        loopback.server.deployer_auth.env.as_deref(),
        Some("OPEN_COMPUTE_DEPLOYER_TOKEN")
    );
    assert_eq!(
        loopback.server.read_only_auth.env.as_deref(),
        Some("OPEN_COMPUTE_READ_ONLY_TOKEN")
    );
}

#[test]
fn secret_refs_must_be_mutually_valid() {
    let err = parse_err("[storage]\nbackend = \"s3\"\naccess_key_id_env = \"\"\n");
    assert_eq!(err.code(), ErrorCode::SecretRefInvalid);
    let err = parse_err("[storage]\nbackend = \"s3\"\naccess_key_id_env = \"lowercase\"\n");
    assert_eq!(err.code(), ErrorCode::SecretRefInvalid);
    let both = parse_ok(
        r#"
[storage]
backend = "s3"
access_key_id_env = "S3_ACCESS_KEY_ID"
access_key_id_file = "/var/lib/open-compute/keys/s3-access"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
secret_access_key_file = "/var/lib/open-compute/keys/s3-secret"
"#,
    );
    assert!(s3(&both).access_key_id_file.is_some());
}

#[test]
fn object_storage_variants_and_day1_wire_shape_are_strict() {
    for input in [
        "[storage]\nbackend = \"local\"\nendpoint = \"https://s3.example.com\"\n",
        "[storage]\nbackend = \"s3\"\npath = \"/var/lib/open-compute/objects\"\n",
        "[storage]\ndata_dir = \"/var/lib/open-compute\"\n",
        "[object_storage]\nbackend = \"local\"\npath = \"/var/lib/open-compute/objects\"\n",
        "[s3]\nendpoint = \"https://s3.example.com\"\n",
    ] {
        assert_eq!(
            parse_err(input).code(),
            ErrorCode::ConfigParseFailed,
            "{input}"
        );
    }
}

#[test]
fn local_object_root_accepts_only_reserved_or_disjoint_layouts() {
    assert!(parse_ok("").object_storage.as_local().is_some());
    let disjoint =
        parse_ok("[storage]\nbackend = \"local\"\npath = \"/srv/open-compute-objects\"\n");
    assert_eq!(
        disjoint.object_storage.as_local().unwrap().path,
        Path::new("/srv/open-compute-objects")
    );
    for path in [
        "/var/lib/open-compute",
        "/var/lib",
        "/var/lib/open-compute/cache/objects",
        "/var/lib/open-compute/keys",
    ] {
        let input = format!("[storage]\nbackend = \"local\"\npath = {path:?}\n");
        assert_eq!(parse_err(&input).code(), ErrorCode::PathInvalid, "{path}");
    }
}

#[test]
fn object_storage_prefix_and_s3_timeout_bounds() {
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nprefix = \"system\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nprefix = \"/system/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nprefix = \"../system/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nprefix = \"tenant/foo/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nr2_prefix = \"system/r2/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nr2_prefix = \"tenant//r2/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"local\"\nprefix = \"systèm/\"\n").code(),
        ErrorCode::ObjectStoragePrefixInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nmax_retries = 0\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err(
            "[storage]\nbackend = \"s3\"\nconnect_timeout_ms = 10000\nrequest_timeout_ms = 1000\n"
        )
        .code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nendpoint = \"not-a-url\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nverify_tls = false\n").code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nendpoint = \"https://user:pass@s3.example.com\"\n")
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nendpoint = \"https://s3.example.com/?x=1\"\n")
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nendpoint = \"https://s3.example.com/#frag\"\n")
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_err("[storage]\nbackend = \"s3\"\nendpoint = \"ftp://s3.example.com\"\n").code(),
        ErrorCode::ConfigInvalid
    );
    let local = parse_ok("[storage]\nbackend = \"s3\"\nendpoint = \"http://127.0.0.1:9000\"\n");
    assert_eq!(s3(&local).endpoint, "http://127.0.0.1:9000");
    assert!(s3(&local).verify_tls);
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
fn removed_config_surfaces_are_rejected() {
    assert_eq!(
        parse_err("[server]\ntrusted_proxies = [\"10.0.0.0/8\"]\n").code(),
        ErrorCode::ConfigParseFailed
    );
    assert_eq!(
        parse_err("[diagnostics]\nmax_failed_starts = 32\nmax_bytes = 16777216\n").code(),
        ErrorCode::ConfigParseFailed
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
        parse_err("[data]\nfree_space_hard_bytes = 99\nfree_space_soft_bytes = 1\n").code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        parse_err("[metrics]\nmax_series = 0\n").code(),
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
[storage]
backend = "s3"
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
"#,
    );
    let debug = format!("{config:?}");
    assert!(!debug.contains("AKIA"));
    assert_eq!(
        s3(&config).access_key_id_env.as_deref(),
        Some("S3_ACCESS_KEY_ID")
    );
}

#[test]
fn file_only_s3_credentials_drop_implicit_default_env_names() {
    let config = parse_ok(
        r#"
[storage]
backend = "s3"
access_key_id_file = "/var/lib/open-compute/keys/s3-access.key"
secret_access_key_file = "/var/lib/open-compute/keys/s3-secret.key"
"#,
    );
    assert!(s3(&config).access_key_id_env.is_none());
    assert!(s3(&config).secret_access_key_env.is_none());
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
        "[data]\nmaster_key_env = \"9BAD\"\n",
        "[data]\nsqlite_busy_timeout_ms = 0\n",
        "[storage]\nbackend = \"s3\"\nregion = \"\"\n",
        "[storage]\nbackend = \"s3\"\nbucket = \"\"\n",
        "[runtime]\nrestart_backoff_initial_ms = 0\n",
        "[runtime]\nrestart_backoff_max_ms = 0\n",
        "[workers]\nmax_bundle_bytes = 0\n",
        "[workers]\nmax_request_body_bytes = 0\n",
        "[workers]\ndelete_drain_timeout_ms = 0\n",
        "[workers]\nartifact_gc_grace_ms = 0\n",
        "[workers]\nartifact_gc_interval_ms = 0\n",
        "[workers]\ndelete_recovery_batch = 0\n",
        "[workers]\nretain_ready_versions = 0\n",
        "[workers]\nretain_rejected_versions = 0\n",
        "[workers]\nversion_min_retention_ms = 0\n",
        "[workers]\nmax_bundle_bytes = 67108865\n",
        "[workers]\ndelete_recovery_batch = 10001\n",
        "[scheduler]\npoll_interval_ms = 0\n",
        "[scheduler]\nmax_in_flight = 0\n",
        "[scheduler]\nclaim_lease_ms = 34999\n",
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
        let input = format!("[storage]\nbackend = \"s3\"\nendpoint = {endpoint:?}\n");
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
}

#[test]
fn scheduler_pool_defaults_are_independent_of_global_admission() {
    let defaults = parse_ok("[scheduler]\nmax_in_flight = 7\nclaim_lease_ms = 60000\n");
    assert_eq!(defaults.scheduler.pools, SchedulerPoolsConfig::default());
    assert_eq!(
        defaults.scheduler.pool(SchedulerKind::Alarm),
        SchedulerPoolConfig {
            enabled: true,
            max_in_flight: 16,
            claim_batch: 32,
            weight: 1,
        }
    );
    assert_eq!(
        parse_err("[scheduler]\nclaim_batch = 12\n").code(),
        ErrorCode::ConfigParseFailed
    );
    assert_eq!(
        toml::from_str::<SchedulerConfig>(&toml::to_string(&defaults.scheduler).unwrap()).unwrap(),
        defaults.scheduler
    );

    let configured = parse_ok(
        "[scheduler.pools.alarm]\nenabled = true\nmax_in_flight = 5\nclaim_batch = 9\nweight = 2\n",
    );
    assert_eq!(
        configured.scheduler.pool(SchedulerKind::Alarm),
        SchedulerPoolConfig {
            enabled: true,
            max_in_flight: 5,
            claim_batch: 9,
            weight: 2,
        }
    );
    let queue = parse_ok("[scheduler.pools.queue]\nenabled = true\n");
    assert!(queue.scheduler.pool(SchedulerKind::Queue).enabled);
    let cron = parse_ok("[scheduler.pools.cron]\nenabled = true\n");
    assert!(cron.scheduler.pool(SchedulerKind::Cron).enabled);
    let workflow = parse_ok("[scheduler.pools.workflow]\nenabled = true\n");
    assert!(workflow.scheduler.pool(SchedulerKind::Workflow).enabled);
    for (name, kind) in [
        ("alarm", SchedulerKind::Alarm),
        ("queue", SchedulerKind::Queue),
        ("cron", SchedulerKind::Cron),
        ("workflow", SchedulerKind::Workflow),
    ] {
        let partial = parse_ok(&format!("[scheduler.pools.{name}]\nweight = 3\n"));
        let expected = SchedulerPoolConfig {
            weight: 3,
            ..SchedulerConfig::default().pool(kind)
        };
        assert_eq!(partial.scheduler.pool(kind), expected);
        assert_eq!(
            parse_err(&format!("[scheduler.pools.{name}]\nunknown = 3\n")).code(),
            ErrorCode::ConfigParseFailed
        );
    }
}

#[test]
fn scheduler_pool_hard_bounds_fail_closed() {
    for input in [
        "[scheduler.pools.alarm]\nmax_in_flight = 0\n",
        "[scheduler.pools.alarm]\nclaim_batch = 0\n",
        "[scheduler.pools.alarm]\nweight = 0\n",
        "[scheduler.pools.alarm]\nmax_in_flight = 4097\n",
        "[scheduler.pools.alarm]\nclaim_batch = 10001\n",
        "[scheduler.pools.alarm]\nweight = 1025\n",
    ] {
        assert_eq!(parse_err(input).code(), ErrorCode::LimitInvalid);
    }
}

#[test]
fn p1_hardening_and_remaining_static_error_paths_are_validated() {
    for input in [
        "[hardening]\nmax_workers_per_account = 0\n",
        "[hardening]\nmax_routes_per_account = 10000001\n",
        "[hardening]\nmax_versions_per_worker = 0\n",
        "[hardening]\nmax_resources_per_kind_per_account = 0\n",
        "[hardening]\nemergency_reserve_bytes = 0\n",
        "[hardening]\nmax_snapshot_files = 0\n",
        "[hardening]\nmax_snapshot_file_bytes = 0\n",
        "[hardening]\nmax_snapshot_file_bytes = 2\nmax_snapshot_total_bytes = 1\n",
        "[hardening]\nmax_snapshot_manifest_bytes = 67108865\n",
        "[hardening]\nsnapshot_staging_margin_bytes = 0\n",
        "[hardening]\nincomplete_snapshot_grace_ms = 0\n",
        "[hardening]\nsnapshot_stale_after_ms = 0\n",
        "[hardening]\nmax_support_bundle_bytes = 0\n",
        "[hardening]\nemergency_reserve_bytes = 268435456\n",
        "[kv]\nnamespace_quota_bytes = 1\n",
        "[queues]\ndefault_max_backlog_bytes = 0\n",
        "[queues]\nmax_in_flight_requests = 0\n",
        "[queues]\nmax_in_flight_requests = 2\nmax_in_flight_requests_per_binding = 3\n",
        "[server]\npublic_bind = \"not-an-address\"\n",
        "[server]\nadmin_bind = \"not-an-address\"\n",
        "[server]\nadmin_bind = \"0.0.0.0:8788\"\nadmin_auth = { env = \"bad-name\" }\n",
        "[data]\nfree_space_soft_bytes = 0\n",
        "[data]\nfree_space_hard_bytes = 0\n",
        "[storage]\nbackend = \"s3\"\nsecret_access_key_env = \"bad-name\"\n",
        "[storage]\nbackend = \"s3\"\nretry_backoff_ms = 0\n",
        "[storage]\nbackend = \"s3\"\nconnect_timeout_ms = 0\n",
        "[storage]\nbackend = \"s3\"\nrequest_timeout_ms = 0\n",
        "[runtime]\nshutdown_grace_ms = 0\n",
        "[runtime]\ndrain_timeout_ms = 0\n",
        "[runtime]\nkill_timeout_ms = 0\n",
        "[runtime]\nrestart_budget = 0\n",
        "[runtime]\nrestart_window_ms = 0\n",
        "[cache]\nmax_artifact_bytes = 0\n",
        "[metrics]\nmax_label_value_bytes = 0\n",
        "[diagnostics]\nmax_bytes = 0\n",
        "[scheduler]\ndispatch_timeout_ms = 18446744073709551615\nlease_guard_ms = 1\n",
        "[storage]\nbackend = \"s3\"\nendpoint = \"https:///\"\n",
    ] {
        let _ = parse_err(input);
    }

    let config = ServerConfig {
        admin_bind: Some("not-an-address".to_owned()),
        ..ServerConfig::default()
    };
    assert!(config.admin_addr().is_err());
    assert!(
        validate_secret_pair(Some("bad-name"), Some(Path::new("/tmp/secret")), "test").is_err()
    );
}
