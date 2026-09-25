use super::*;
use crate::exit::ExitClass;
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::service_manager::FakeServiceManager;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::SystemTime;
use tempfile::TempDir;

const TEST_INSTANCE_ID: &str = "01890f3c8b407cc0a000000000000001";

#[test]
fn cache_clean_cli_selectors_are_exclusive() {
    assert!(parse_from(["ocd", "cache", "clean"]).is_ok());
    assert!(parse_from(["ocd", "cache", "clean", "--dry-run", "--all"]).is_ok());
    assert!(parse_from(["ocd", "cache", "clean", "--instance", "dev"]).is_ok());
    assert!(parse_from(["ocd", "cache", "clean", "--instance", "dev", "--all"]).is_err());
}

#[tokio::test]
async fn daemon_setup_rejects_instance_path_selection_before_writing() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = temp.path().join("project/compute.toml");
    let cli = parse_from([
        "ocd",
        "--no-update-check",
        "setup",
        "--config",
        config.to_str().unwrap(),
        "--yes",
    ])
    .unwrap();
    let mut errors = Vec::new();
    let exit = execute_with_deps(cli, &mut Vec::new(), &mut errors, temp.path(), &deps).await;
    assert_ne!(exit, ExitCode::SUCCESS);
    assert!(
        String::from_utf8(errors)
            .unwrap()
            .contains("CONFIG_PATH_INVALID")
    );
    assert!(!config.exists());
    assert!(
        !deps
            .registry
            .root_for(ServiceScope::User)
            .join("ocd.toml")
            .exists()
    );
}

#[tokio::test]
async fn offline_cache_clean_keeps_global_and_instance_scopes_separate() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let root = deps.registry.root_for(ServiceScope::User);
    let config = temp.path().join("compute.toml");
    let data = temp.path().join("instance-data");
    let name = "dev".parse().unwrap();
    crate::setup::create_instance(root, ServiceScope::User, &config, &data, Some(&name)).unwrap();
    let record = deps
        .registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    drop(crate::run::DaemonLock::acquire(root).unwrap());
    let shared_cache = root.join("cache");
    fs::create_dir_all(&shared_cache).unwrap();
    let global_entry = shared_cache.join("update-check.json");
    fs::write(&global_entry, b"shared").unwrap();
    let shard = data.join("cache/artifacts/sha256/ab");
    fs::create_dir_all(&shard).unwrap();
    let instance_entry = shard.join("ab".repeat(31));
    fs::write(&instance_entry, b"instance").unwrap();
    let scope_lock_before = fs::read(root.join("ocd.lock")).unwrap();
    let instance_lock_before = fs::read(data.join("platform.lock")).unwrap();

    let (code, out, err) = run_cache_cli(
        vec!["ocd", "cache", "clean", "--all", "--dry-run"],
        &temp,
        &deps,
    )
    .await;
    assert_eq!(code, ExitCode::SUCCESS, "{err}\n{out}");
    assert!(out.contains("target=global"));
    assert!(out.contains(&format!("target={}", record.instance_id)));
    assert!(global_entry.exists() && instance_entry.exists());
    assert_eq!(fs::read(root.join("ocd.lock")).unwrap(), scope_lock_before);
    assert_eq!(
        fs::read(data.join("platform.lock")).unwrap(),
        instance_lock_before
    );

    let held = crate::run::DaemonLock::acquire_existing(root).unwrap();
    let (code, _, _) = run_cache_cli(
        vec!["ocd", "cache", "clean", "--all", "--dry-run"],
        &temp,
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::SUCCESS);
    assert!(global_entry.exists() && instance_entry.exists());
    drop(held);

    let (code, _, err) = run_cache_cli(
        vec!["ocd", "cache", "clean", "--instance", "dev"],
        &temp,
        &deps,
    )
    .await;
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!instance_entry.exists());
    assert!(global_entry.exists());

    let (code, _, err) = run_cache_cli(vec!["ocd", "cache", "clean"], &temp, &deps).await;
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!global_entry.exists());

    fs::write(&global_entry, b"shared").unwrap();
    fs::write(&instance_entry, b"instance").unwrap();
    let (code, out, err) =
        run_cache_cli(vec!["ocd", "cache", "clean", "--all"], &temp, &deps).await;
    assert_eq!(code, ExitCode::SUCCESS, "{err}\n{out}");
    assert!(
        out.contains("target=global") && out.contains(&format!("target={}", record.instance_id))
    );
    assert!(!global_entry.exists() && !instance_entry.exists());
}

async fn run_cache_cli(
    args: Vec<&str>,
    temp: &TempDir,
    deps: &OperatorDeps,
) -> (ExitCode, String, String) {
    let cli = parse_from(args).unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute_with_deps(cli, &mut out, &mut err, temp.path(), deps).await;
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn write_mode(path: &Path, body: &str, mode: u32) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_loadable_config(dir: &Path) -> PathBuf {
    let data = dir.join("data");
    let objects = dir.join("objects");
    let admin = dir.join("admin.token");
    let deployer = dir.join("deployer.token");
    let read_only = dir.join("read-only.token");
    let master = dir.join("master.key");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&objects).unwrap();
    write_mode(&admin, "admin-secret-value\n", 0o600);
    write_mode(&deployer, "deployer-secret-value\n", 0o600);
    write_mode(&read_only, "read-only-secret-value\n", 0o600);
    let toml = format!(
        r#"
[auth]
deployer_auth = {{ file = "{deployer}" }}
read_only_auth = {{ file = "{read_only}" }}

[data]
path = "{data}"
master_key_file = "{master}"

[storage]
backend = "local"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
        deployer = deployer.display(),
        read_only = read_only.display(),
        data = data.display(),
        master = master.display(),
    );
    let path = dir.join("compute.toml");
    fs::write(&path, toml).unwrap();
    path
}

fn test_deps(temp: &TempDir) -> OperatorDeps {
    OperatorDeps {
        registry: InstanceRegistry::with_roots(
            temp.path().join("registry/system"),
            temp.path().join("registry/user"),
        ),
        manager: Arc::new(FakeServiceManager::default()),
        targets: TargetRegistry::at(temp.path().join("targets/targets.toml")),
        target_http: Arc::new(LiveTargetHttp::new().unwrap()),
    }
}

fn write_shared_gateway(registry: &InstanceRegistry) {
    let root = registry.root_for(ServiceScope::User);
    fs::create_dir_all(root).unwrap();
    let path = root.join("ocd.toml");
    let source = fs::read_to_string(&path).unwrap_or_default();
    write_mode(
        &path,
        &format!(
            "{source}\n[gateway]\ningress_ipv4 = [\"203.0.113.10\"]\nhttps_listen = \"127.0.0.1:8443\"\nchallenge_dns_listen = \"127.0.0.1:8053\"\n"
        ),
        0o600,
    );
}

fn initialize_config(path: &Path) {
    let loaded = load_platform_config_from(path, Path::new("/")).unwrap();
    drop(
        open_compute_storage::PlatformStorage::bootstrap_with_hardening(
            &loaded.config.data,
            &loaded.config.hardening,
            &open_compute_core::clock::SystemClock,
        )
        .unwrap(),
    );
}

#[test]
fn parse_from_covers_operator_subcommands() {
    type Case = (&'static [&'static str], fn(&Command) -> bool);
    let cases: &[Case] = &[
        (&["ocd", "start"], |c| matches!(c, Command::Start)),
        (&["ocd", "stop"], |c| matches!(c, Command::Stop)),
        (&["ocd", "restart"], |c| matches!(c, Command::Restart)),
        (&["ocd", "status", "--json"], |c| {
            matches!(c, Command::Status { json: true })
        }),
        (&["ocd", "logs", "--follow"], |c| {
            matches!(c, Command::Logs { follow: true })
        }),
        (&["ocd", "dashboard", "--no-open", "--json"], |c| {
            matches!(
                c,
                Command::Dashboard {
                    no_open: true,
                    json: true
                }
            )
        }),
        (&["ocd", "setup", "--yes"], |c| {
            matches!(c, Command::Setup { yes: true })
        }),
        (&["ocd", "setup", "--system", "--yes"], |c| {
            matches!(c, Command::Setup { yes: true })
        }),
        (&["ocd", "instances"], |c| {
            matches!(c, Command::Instances { json: false })
        }),
        (&["ocd", "instance", "remove", TEST_INSTANCE_ID], |c| {
            matches!(
                c,
                Command::Instance {
                    command: InstanceCommand::Remove { .. }
                }
            )
        }),
        (
            &["ocd", "instance", "add", "--config", "/tmp/compute.toml"],
            |c| {
                matches!(
                    c,
                    Command::Instance {
                        command: InstanceCommand::Add
                    }
                )
            },
        ),
        (&["ocd", "licenses"], |c| matches!(c, Command::Licenses)),
        (&["ocd", "docs"], |c| {
            matches!(c, Command::Docs { name: None })
        }),
        (
            &["ocd", "docs", "install"],
            |c| matches!(c, Command::Docs { name: Some(n) } if n == "install"),
        ),
        (&["ocd", "worker", "bundle"], |c| {
            matches!(
                c,
                Command::Worker {
                    command: WorkerCommand::Bundle
                }
            )
        }),
        (&["ocd", "capabilities", "--json"], |c| {
            matches!(c, Command::Capabilities { json: true })
        }),
        (&["ocd", "target", "list", "--json"], |c| {
            matches!(
                c,
                Command::Target {
                    command: TargetCommand::List { json: true }
                }
            )
        }),
        (
            &[
                "ocd",
                "target",
                "add",
                "remote",
                "--api-base-url",
                "https://compute.example/client/v4",
                "--instance-id",
                "01890f3c8b407cc0a000000000000001",
                "--token-file",
                "/secure/deployer.token",
            ],
            |c| {
                matches!(
                    c,
                    Command::Target {
                        command: TargetCommand::Add { .. }
                    }
                )
            },
        ),
        (
            &[
                "ocd",
                "wrangler",
                "--target",
                "remote",
                "--project",
                "/srv/worker",
                "deploy",
                "--config",
                "./wrangler.jsonc",
                "--unknown",
                "值",
            ],
            |c| {
                matches!(
                    c,
                    Command::Wrangler { target: Some(target), project: Some(project), arguments }
                        if target.as_str() == "remote"
                            && project == Path::new("/srv/worker")
                            && arguments == &["deploy", "--config", "./wrangler.jsonc", "--unknown", "值"]
                )
            },
        ),
        (&["ocd", "wrangler", "--", "--version"], |c| {
            matches!(
                c,
                Command::Wrangler { arguments, .. }
                    if arguments == &["--version"]
            )
        }),
        (
            &["ocd", "wrangler", "--instance", TEST_INSTANCE_ID, "deploy"],
            |c| {
                matches!(
                    c,
                    Command::Wrangler { arguments, .. }
                        if arguments == &["deploy"]
                )
            },
        ),
    ];
    for (args, check) in cases {
        let parsed = parse_from(*args).unwrap_or_else(|err| panic!("{args:?}: {err}"));
        assert!(check(&parsed.command), "{args:?}");
    }
    assert!(
        parse_from([
            "ocd",
            "wrangler",
            "--target",
            "remote",
            "--instance",
            TEST_INSTANCE_ID,
            "deploy",
        ])
        .is_err()
    );
}

#[test]
fn instance_add_uses_the_global_config_argument() {
    let cli = parse_from(["ocd", "instance", "add", "--config", "/tmp/compute.toml"]).unwrap();
    assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/compute.toml")));
    assert!(matches!(
        cli.command,
        Command::Instance {
            command: InstanceCommand::Add
        }
    ));
}

#[test]
fn run_selects_scope_without_instance_config() {
    let user = parse_from(["ocd", "run"]).unwrap();
    assert!(matches!(user.command, Command::Run));
    assert!(!user.system);
    let system = parse_from(["ocd", "run", "--system"]).unwrap();
    assert!(matches!(system.command, Command::Run));
    assert!(system.system);
    assert!(
        parse_from(["ocd", "setup", "--system", "--yes"])
            .unwrap()
            .system
    );
}

#[test]
fn write_instances_empty_and_listed() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut out = Vec::new();
    crate::instance_ops::write_instances(&deps.registry, ServiceScope::User, &mut out, false)
        .unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("No registered instances")
    );

    let config = write_loadable_config(temp.path());
    initialize_config(&config);
    deps.registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let mut out = Vec::new();
    crate::instance_ops::write_instances(&deps.registry, ServiceScope::User, &mut out, false)
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("ID  NAME  STATE"));
    assert!(text.contains("stopped"));

    let mut out = Vec::new();
    crate::instance_ops::write_instances(&deps.registry, ServiceScope::User, &mut out, true)
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["command"], "instances");
    assert_eq!(payload["instances"].as_array().unwrap().len(), 1);
}

#[test]
fn write_instances_rejects_unresponsive_locked_daemon() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let root = deps.registry.root_for(ServiceScope::User);
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    open_compute_storage::ensure_dir_secure(root).unwrap();
    let lock = root.join("ocd.lock");
    write_mode(&lock, "", 0o600);
    let file = fs::File::open(&lock).unwrap();
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let error = crate::instance_ops::write_instances(
        &deps.registry,
        ServiceScope::User,
        &mut Vec::new(),
        false,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
    let error = crate::instance_ops::write_daemon_status(
        &deps.registry,
        ServiceScope::User,
        &mut Vec::new(),
        false,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
}

#[test]
fn scoped_status_is_offline_without_creating_data() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut out = Vec::new();
    crate::instance_ops::write_daemon_status(&deps.registry, ServiceScope::User, &mut out, true)
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["state"], "stopped");
    assert!(!deps.registry.root_for(ServiceScope::User).exists());
}

#[test]
fn scoped_status_and_listing_reject_instance_selection() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    for command in ["status", "instances"] {
        let cli = parse_from(["ocd", "--instance", TEST_INSTANCE_ID, command]).unwrap();
        let error = daemon::run_scope_command(&cli, Some(&deps), &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigPathInvalid);
    }
}

#[test]
fn instance_setup_accepts_independent_config_and_data_choices() {
    let cli = parse_from([
        "ocd",
        "--config",
        "/projects/dev/compute.toml",
        "instance",
        "setup",
        "--name",
        "dev",
        "--data-dir",
        "/mnt/data/dev",
        "--yes",
        "--autostart=false",
        "--start=false",
    ])
    .unwrap();
    let Command::Instance {
        command:
            InstanceCommand::Setup {
                name,
                data_dir,
                yes,
                autostart,
                start,
            },
    } = cli.command
    else {
        panic!("expected instance setup");
    };
    assert_eq!(name.unwrap().as_str(), "dev");
    assert_eq!(data_dir.unwrap(), PathBuf::from("/mnt/data/dev"));
    assert!(yes);
    assert!(!autostart && !start);
}

#[test]
fn write_config_check_human_and_json() {
    let mut out = Vec::new();
    write_config_check(&mut out, false).unwrap();
    assert_eq!(String::from_utf8(out).unwrap().trim(), "CONFIG_OK");
    let mut out = Vec::new();
    write_config_check(&mut out, true).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["result"], "ok");
    assert_eq!(payload["command"], "config_check");
}

#[test]
fn gateway_dns_plan_reports_exact_records_and_ports() {
    assert!(matches!(
        parse_from(["ocd", "config", "gateway-dns-plan", "--json"])
            .unwrap()
            .command,
        Command::Config {
            command: ConfigCommand::GatewayDnsPlan { json: true }
        }
    ));
    let gateway = PublicGatewayConfig {
        base_domain: "compute.example.com".into(),
        shared: open_compute_core::DaemonGatewayConfig {
            ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
            ingress_ipv6: Vec::new(),
            https_listen: "127.0.0.1:8443".parse().unwrap(),
            challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
            proxy_protocol_from: Vec::new(),
            caddy: Vec::new(),
        },
    };
    let mut human = Vec::new();
    write_gateway_dns_plan(&mut human, &gateway, false).unwrap();
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("*.compute.example.com CNAME ingress.compute.example.com"));
    assert!(human.contains("_acme-challenge.compute.example.com NS ns1.compute.example.com"));
    assert!(human.contains("Inbound: TCP 443, UDP 53, TCP 53"));
    let mut json = Vec::new();
    write_gateway_dns_plan(&mut json, &gateway, true).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(value["records"].as_array().unwrap().len(), 4);
    assert_eq!(
        value["inbound_ports"],
        serde_json::json!(["tcp/443", "udp/53", "tcp/53"])
    );
    assert!(matches!(
        parse_from(["ocd", "config", "gateway-challenge-probe", "--json"])
            .unwrap()
            .command,
        Command::Config {
            command: ConfigCommand::GatewayChallengeProbe { json: true }
        }
    ));
    assert!(matches!(
        parse_from(["ocd", "config", "gateway-dns-verify", "--json"])
            .unwrap()
            .command,
        Command::Config {
            command: ConfigCommand::GatewayDnsVerify { json: true, .. }
        }
    ));
    assert!(matches!(
        parse_from(["ocd", "config", "gateway-tls-probe", "--json"])
            .unwrap()
            .command,
        Command::Config {
            command: ConfigCommand::GatewayTlsProbe { json: true }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "config",
        "gateway-dns-verify",
        "--resolver",
        "1.1.1.1:53",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Config {
            command: ConfigCommand::GatewayDnsVerify { resolver, .. }
        } if resolver == ["1.1.1.1:53".parse().unwrap()]
    ));
}

#[test]
fn resolve_loaded_config_rejects_mutual_exclusive() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let selector: InstanceSelector = TEST_INSTANCE_ID.parse().unwrap();
    let err = resolve_loaded_config(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        Some(&deps.registry),
        ServiceScope::User,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
}

#[test]
fn default_config_selection_uses_only_registered_instances() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    let error = resolve_loaded_config(
        None,
        None,
        temp.path(),
        Some(&deps.registry),
        ServiceScope::User,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceNotFound);
    initialize_config(&config);
    deps.registry
        .register(&config, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert_eq!(
        resolve_loaded_config(
            None,
            None,
            temp.path(),
            Some(&deps.registry),
            ServiceScope::User,
        )
        .unwrap()
        .path,
        fs::canonicalize(config).unwrap()
    );
}

#[tokio::test]
async fn offline_restore_checks_the_same_ocd_data_boundary_as_registration() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let root = deps.registry.root_for(ServiceScope::User);
    let allowed_dir = root.join("instances/dev");
    fs::create_dir_all(&allowed_dir).unwrap();
    let allowed = write_loadable_config(&allowed_dir);
    assert!(
        resolve_loaded_config(
            Some(&allowed),
            None,
            temp.path(),
            Some(&deps.registry),
            ServiceScope::User,
        )
        .is_ok()
    );

    let denied_dir = root.join("cache/restore");
    fs::create_dir_all(&denied_dir).unwrap();
    let denied = write_loadable_config(&denied_dir);
    assert_eq!(
        resolve_loaded_config(
            Some(&denied),
            None,
            temp.path(),
            Some(&deps.registry),
            ServiceScope::User,
        )
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );
    let cli = parse_from([
        "ocd",
        "--no-update-check",
        "--config",
        denied.to_str().unwrap(),
        "backup",
        "restore",
        "--snapshot",
        "snapshot-id",
    ])
    .unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    assert_ne!(
        execute_with_deps(cli, &mut out, &mut err, temp.path(), &deps).await,
        ExitCode::SUCCESS
    );
    assert!(String::from_utf8_lossy(&err).contains("PATH_INVALID"));
    assert!(!denied_dir.join("data/control.sqlite").exists());

    let _daemon = crate::run::DaemonLock::acquire(root).unwrap();
    let cli = parse_from([
        "ocd",
        "--no-update-check",
        "--config",
        allowed.to_str().unwrap(),
        "backup",
        "restore",
        "--snapshot",
        "snapshot-id",
    ])
    .unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    assert_ne!(
        execute_with_deps(cli, &mut out, &mut err, temp.path(), &deps).await,
        ExitCode::SUCCESS
    );
    assert!(String::from_utf8_lossy(&err).contains("INSTANCE_REGISTRY_INVALID"));
    assert!(!allowed_dir.join("data/control.sqlite").exists());
}

#[test]
fn offline_interrupted_message() {
    let err = offline_interrupted();
    assert_eq!(err.code(), ErrorCode::PlatformUnavailable);
    assert!(err.message().contains("interrupted"));
}

#[tokio::test]
async fn execute_instances_and_operator_lifecycle() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    let config_str = config.to_str().unwrap();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "instances"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("No registered instances")
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "--config", config_str, "start"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        deps.registry
            .list_scope(ServiceScope::User)
            .unwrap()
            .is_empty()
    );
    initialize_config(&config);
    deps.registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let listed = deps.registry.list_scope(ServiceScope::User).unwrap();
    assert_eq!(listed.len(), 1);
    let id = listed[0].instance_id.clone();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "status", "--json"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    let payload: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(payload["command"], "status");
    assert_eq!(payload["state"], "stopped");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "--instance", &id, "logs"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(stdout.is_empty());

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "--instance", &id, "restart"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "--instance", &id, "stop"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert_eq!(
        deps.registry.list_scope(ServiceScope::User).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn execute_rejects_setup_and_run_with_instance() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--instance",
            TEST_INSTANCE_ID,
            "setup",
            "--yes",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("does not accept --config or --instance")
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--instance",
            TEST_INSTANCE_ID,
            "run",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("selects only the user or explicit system OCD scope")
    );
}

#[tokio::test]
async fn execute_licenses_docs_config_init_and_check() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    let config_str = config.to_str().unwrap();

    for args in [
        vec!["ocd", "--no-update-check", "licenses"],
        vec!["ocd", "--no-update-check", "docs"],
        vec![
            "ocd",
            "--no-update-check",
            "config",
            "init",
            "--data-dir",
            "/tmp/oc-data",
        ],
    ] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute_with_deps(
            parse_from(args.clone()).unwrap(),
            &mut stdout,
            &mut stderr,
            temp.path(),
            &deps,
        )
        .await;
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "{args:?} stderr={}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(!stdout.is_empty(), "{args:?}");
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            config_str,
            "config",
            "check",
            "--json",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    let payload: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(payload["command"], "config_check");
}

#[tokio::test]
async fn execute_dashboard_fails_when_not_ready() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    initialize_config(&config);
    deps.registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let id = deps.registry.list_scope(ServiceScope::User).unwrap()[0]
        .instance_id
        .clone();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--instance",
            &id,
            "dashboard",
            "--no-open",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
}

#[tokio::test]
async fn execute_doctor_basic_on_bootstrapped_config() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    let _storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    drop(_storage);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            config.to_str().unwrap(),
            "doctor",
            "--json",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert!(
        code == ExitCode::from(ExitClass::Ok.code())
            || code == ExitCode::from(ExitClass::Doctor.code()),
        "stderr={}",
        String::from_utf8_lossy(&stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(payload["command"], "doctor");
}

#[tokio::test]
async fn execute_setup_rejects_system_with_config() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            "project/compute.toml",
            "setup",
            "--system",
            "--yes",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("does not accept --config or --instance")
    );
}

#[test]
fn operator_deps_required_matrix() {
    assert!(operator_deps_required(
        &parse_from(["ocd", "start"]).unwrap()
    ));
    assert!(operator_deps_required(
        &parse_from(["ocd", "upgrade"]).unwrap()
    ));
    assert!(!operator_deps_required(
        &parse_from(["ocd", "licenses"]).unwrap()
    ));
    assert!(!operator_deps_required(
        &parse_from(["ocd", "doctor"]).unwrap()
    ));
    assert!(operator_deps_required(
        &parse_from(["ocd", "--instance", TEST_INSTANCE_ID, "doctor"]).unwrap()
    ));
}

#[test]
fn parse_upgrade_uninstall_update_check() {
    let cli = parse_from(["ocd", "upgrade", "0.2.0", "--dry-run"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Upgrade {
            dry_run: true,
            version: Some(ref v),
            ..
        } if v == "0.2.0"
    ));
    assert!(matches!(
        parse_from(["ocd", "uninstall"]).unwrap().command,
        Command::Uninstall {
            purge: false,
            yes: false,
            dry_run: false
        }
    ));
    assert!(matches!(
        parse_from(["ocd", "uninstall", "--purge", "--yes"])
            .unwrap()
            .command,
        Command::Uninstall {
            purge: true,
            yes: true,
            dry_run: false
        }
    ));
    assert!(parse_from(["ocd", "uninstall", "--yes"]).is_err());
    assert!(matches!(
        parse_from(["ocd", "__update_check"]).unwrap().command,
        Command::UpdateCheck
    ));
    assert!(matches!(
        parse_from(["ocd", "purge", "--instance", TEST_INSTANCE_ID, "--dry-run",])
            .unwrap()
            .command,
        Command::Purge {
            yes: false,
            dry_run: true
        }
    ));
    assert!(parse_from(["ocd", "instance", "remove", "--instance", TEST_INSTANCE_ID,]).is_err());
}

#[tokio::test]
async fn upgrade_preflight_requires_one_initialized_explicit_config() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = write_loadable_config(temp.path());
    initialize_config(&config);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            config.to_str().unwrap(),
            "__upgrade_preflight",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&stderr)
    );
    assert_eq!(stdout, b"UPGRADE_PREFLIGHT_OK\n");

    let missing = parse_from(["ocd", "--no-update-check", "__upgrade_preflight"]).unwrap();
    assert_eq!(
        run_upgrade_preflight(&missing, &mut Vec::new(), temp.path())
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    let mut conflicting = parse_from([
        "ocd",
        "--no-update-check",
        "--config",
        config.to_str().unwrap(),
        "__upgrade_preflight",
    ])
    .unwrap();
    conflicting.instance = Some(TEST_INSTANCE_ID.parse().unwrap());
    assert_eq!(
        run_upgrade_preflight(&conflicting, &mut Vec::new(), temp.path())
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
}

#[test]
fn setup_scope_requires_system_mode_for_root() {
    let error = validate_setup_scope(true, false).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    assert!(error.message().contains("--system"));
    validate_setup_scope(true, true).unwrap();
    validate_setup_scope(false, false).unwrap();
}

#[tokio::test]
async fn execute_upgrade_and_uninstall_fail_closed_without_receipt() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "upgrade", "--dry-run"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(!stderr.is_empty() || !stdout.is_empty());

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "uninstall"]).unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
}

#[tokio::test]
async fn execute_setup_rejects_custom_config_without_creating_instance() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let config = temp.path().join("etc/open-compute/config.toml");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            config.to_str().unwrap(),
            "setup",
            "--yes",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("does not accept --config or --instance")
    );
    assert_eq!(
        deps.registry.list_scope(ServiceScope::User).unwrap().len(),
        0
    );
    assert!(!config.exists());
}

#[tokio::test]
async fn execute_production_deps_path_for_instances() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute(
        parse_from(["ocd", "--no-update-check", "instances", "--json"]).unwrap(),
        &mut stdout,
        &mut stderr,
    )
    .await;
    let _ = code;
}

#[tokio::test]
async fn support_helpers_cover_output_scope_and_interruptible_success() {
    assert!(require_operator_deps(None).is_err());
    assert!(validate_setup_scope(true, false).is_err());
    validate_setup_scope(true, true).unwrap();
    assert_eq!(
        interruptible_offline(async { Ok::<_, PlatformError>(7) })
            .await
            .unwrap(),
        7
    );

    let mut output = Vec::new();
    write_config_check(&mut output, false).unwrap();
    write_config_check(&mut output, true).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("CONFIG_OK"));
    assert!(text.contains("config_check"));

    let gateway = PublicGatewayConfig {
        base_domain: "compute.example.com".to_owned(),
        shared: open_compute_core::DaemonGatewayConfig {
            ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
            ingress_ipv6: vec!["2001:4860:4860::8888".parse().unwrap()],
            https_listen: "127.0.0.1:8443".parse().unwrap(),
            challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
            proxy_protocol_from: Vec::new(),
            caddy: Vec::new(),
        },
    };
    let mut output = Vec::new();
    write_gateway_dns_plan(&mut output, &gateway, false).unwrap();
    write_gateway_dns_plan(&mut output, &gateway, true).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Inbound: TCP 443, UDP 53, TCP 53"));
    assert!(text.contains("config_gateway_dns_plan"));
}

#[tokio::test]
async fn execute_target_lifecycle_and_gateway_config_commands() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    fs::create_dir(temp.path().join("targets")).unwrap();
    fs::set_permissions(
        temp.path().join("targets"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let token = temp.path().join("remote.token");
    write_mode(&token, "remote-secret\n", 0o600);
    let token = token.to_str().unwrap();
    let account = "01890f3c8b407cc0a000000000000001";

    for args in [
        vec![
            "ocd",
            "--no-update-check",
            "target",
            "add",
            "remote",
            "--api-base-url",
            "https://remote.example/client/v4",
            "--instance-id",
            account,
            "--token-file",
            token,
        ],
        vec!["ocd", "--no-update-check", "target", "list", "--json"],
        vec![
            "ocd",
            "--no-update-check",
            "target",
            "show",
            "remote",
            "--json",
        ],
        vec!["ocd", "--no-update-check", "target", "remove", "remote"],
    ] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute_with_deps(
            parse_from(args.clone()).unwrap(),
            &mut stdout,
            &mut stderr,
            temp.path(),
            &deps,
        )
        .await;
        assert_eq!(
            code,
            ExitCode::from(ExitClass::Ok.code()),
            "{args:?}: {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(!stdout.is_empty());
    }

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--config",
            "compute.toml",
            "target",
            "list",
        ])
        .unwrap(),
        &mut stdout,
        &mut stderr,
        temp.path(),
        &deps,
    )
    .await;
    assert_ne!(code, ExitCode::SUCCESS);
    assert!(String::from_utf8_lossy(&stderr).contains("does not accept"));

    let config = write_loadable_config(temp.path());
    fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .unwrap()
        .write_all(
            br#"
[public_gateway]
base_domain = "compute.example.com"
"#,
        )
        .unwrap();
    write_shared_gateway(&deps.registry);
    for args in [
        vec!["config", "gateway-dns-plan", "--json"],
        vec!["capabilities", "--json"],
    ] {
        let mut argv = vec![
            "ocd",
            "--no-update-check",
            "--config",
            config.to_str().unwrap(),
        ];
        argv.extend(args);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute_with_deps(
            parse_from(argv).unwrap(),
            &mut stdout,
            &mut stderr,
            temp.path(),
            &deps,
        )
        .await;
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "{}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(!stdout.is_empty());
    }

    for command in [
        "gateway-challenge-probe",
        "gateway-dns-verify",
        "gateway-tls-probe",
    ] {
        let plain = write_loadable_config(&temp.path().join(command));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute_with_deps(
            parse_from([
                "ocd",
                "--no-update-check",
                "--config",
                plain.to_str().unwrap(),
                "config",
                command,
            ])
            .unwrap(),
            &mut stdout,
            &mut stderr,
            temp.path(),
            &deps,
        )
        .await;
        assert_ne!(code, ExitCode::SUCCESS);
        assert!(String::from_utf8_lossy(&stderr).contains("not configured"));
    }
}

#[tokio::test]
async fn gateway_commands_reject_missing_gateway_at_loaded_boundary() {
    let temp = TempDir::new().unwrap();
    let config = write_loadable_config(temp.path());
    let commands = [
        ConfigCommand::GatewayDnsPlan { json: false },
        ConfigCommand::GatewayChallengeProbe { json: false },
        ConfigCommand::GatewayDnsVerify {
            json: false,
            resolver: Vec::new(),
        },
        ConfigCommand::GatewayTlsProbe { json: false },
    ];
    for command in commands {
        let loaded = load_platform_config_from(&config, temp.path()).unwrap();
        let error = run_loaded(
            Command::Config { command },
            loaded,
            ServiceScope::User,
            None,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        assert!(error.message().contains("not configured"));
    }
}

#[tokio::test]
async fn caddy_version_uses_the_embedded_manifest_without_configuration() {
    let cli = parse_from(["ocd", "--no-update-check", "caddy", "version"]).unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run(cli, &mut stdout, &mut stderr, Path::new("/"), None)
        .await
        .unwrap();
    let output = String::from_utf8(stdout).unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(output.contains("v2.11.4"));
    assert!(output.contains("pin v2.11.4-open-compute.1"));
    assert!(stderr.is_empty());

    for command in ["list-modules", "validate", "reload", "status"] {
        assert!(parse_from(["ocd", "caddy", command]).is_ok());
    }
    assert!(parse_from(["ocd", "caddy", "fmt", "site.caddyfile"]).is_ok());
    assert!(parse_from(["ocd", "caddy", "run"]).is_err());
}

#[tokio::test]
async fn offline_caddy_tools_use_the_verified_embedded_binary() {
    let temp = TempDir::new().unwrap();
    let config = write_loadable_config(temp.path());
    let mut file = fs::OpenOptions::new().append(true).open(&config).unwrap();
    use std::io::Write as _;
    writeln!(
        file,
        r#"
[public_gateway]
base_domain = "compute.example.com"
"#
    )
    .unwrap();
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    let registry_root = tempfile::Builder::new()
        .prefix("oc-caddy-")
        .tempdir_in("/tmp")
        .unwrap();
    let registry = InstanceRegistry::with_roots(
        registry_root.path().join("system"),
        registry_root.path().join("user"),
    );
    write_shared_gateway(&registry);
    drop(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );

    let mut modules = Vec::new();
    crate::caddy_cli::run_offline(
        &registry,
        ServiceScope::User,
        CaddyCommand::ListModules,
        &mut modules,
    )
    .await
    .unwrap();
    let modules = String::from_utf8(modules).unwrap();
    assert!(modules.contains("dns.providers.opencompute"));

    let source = temp.path().join("site.caddyfile");
    fs::write(&source, "example.com { respond ok }").unwrap();
    let mut formatted = Vec::new();
    crate::caddy_cli::run_offline(
        &registry,
        ServiceScope::User,
        CaddyCommand::Fmt {
            file: source.clone(),
        },
        &mut formatted,
    )
    .await
    .unwrap();
    assert!(String::from_utf8(formatted).unwrap().contains("respond ok"));
    assert_eq!(
        fs::read_to_string(source).unwrap(),
        "example.com { respond ok }"
    );

    let mut validated = Vec::new();
    crate::caddy_cli::run_offline(
        &registry,
        ServiceScope::User,
        CaddyCommand::Validate,
        &mut validated,
    )
    .await
    .unwrap();
    assert_eq!(validated, b"CADDY_CONFIG_OK\n");
    assert!(
        fs::read_dir(registry.root_for(ServiceScope::User).join("tmp"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn caddy_cli_selects_the_daemon_scope_without_an_instance() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    write_shared_gateway(&deps.registry);
    let cli = parse_from(["ocd", "--no-update-check", "caddy", "list-modules"]).unwrap();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let exit = execute_with_deps(cli, &mut output, &mut errors, temp.path(), &deps).await;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("dns.providers.opencompute")
    );
    assert!(errors.is_empty());

    for (selector, value) in [("--config", "/unrelated-instance"), ("--instance", "dev")] {
        let cli = parse_from([
            "ocd",
            "--no-update-check",
            selector,
            value,
            "caddy",
            "version",
        ])
        .unwrap();
        let mut errors = Vec::new();
        let exit = execute_with_deps(cli, &mut Vec::new(), &mut errors, temp.path(), &deps).await;
        assert_ne!(exit, ExitCode::SUCCESS);
        assert!(String::from_utf8(errors).unwrap().contains("selects only"));
    }
}

#[tokio::test]
async fn online_caddy_tools_use_the_daemon_control_socket() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixListener;

    let temp = tempfile::Builder::new()
        .prefix("oc-caddy-online-")
        .tempdir_in("/tmp")
        .unwrap();
    let registry =
        InstanceRegistry::with_roots(temp.path().join("ocd/system"), temp.path().join("ocd/user"));
    write_shared_gateway(&registry);
    let root = registry.root_for(ServiceScope::User);
    open_compute_storage::ensure_dir_secure(&root.join("cache")).unwrap();
    open_compute_runtime::materialize_embedded_runtime(&root.join("cache")).unwrap();
    let runtime = root.join("run");
    open_compute_storage::ensure_dir_secure(&runtime).unwrap();
    let socket = runtime.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            assert!(request.contains("caddy_"));
            writeln!(stream, r#"{{"ok":true,"gateway_status":{{"schema_version":1,"child_pid":41,"tls_ready":true,"config_sha256":"abc","last_reload":"ok","last_error":null,"dns":"ok"}}}}"#).unwrap();
        }
    });

    let mut modules = Vec::new();
    crate::caddy_cli::run_offline(
        &registry,
        ServiceScope::User,
        CaddyCommand::ListModules,
        &mut modules,
    )
    .await
    .unwrap();
    assert!(
        String::from_utf8(modules)
            .unwrap()
            .contains("dns.providers.opencompute")
    );
    assert!(fs::read_dir(root.join("tmp")).unwrap().next().is_none());

    for command in [
        CaddyCommand::Status,
        CaddyCommand::Reload,
        CaddyCommand::Validate,
    ] {
        let mut output = Vec::new();
        crate::caddy_cli::run_offline(&registry, ServiceScope::User, command, &mut output)
            .await
            .unwrap();
        assert!(!output.is_empty());
    }
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
}

#[tokio::test]
async fn caddy_tools_fail_closed_without_required_inputs() {
    let temp = TempDir::new().unwrap();
    let config = write_loadable_config(temp.path());
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    drop(DataDir::acquire(&loaded.config.data).unwrap());
    let registry =
        InstanceRegistry::with_roots(temp.path().join("ocd/system"), temp.path().join("ocd/user"));

    for command in [
        CaddyCommand::Status,
        CaddyCommand::Validate,
        CaddyCommand::Fmt {
            file: temp.path().join("missing.caddyfile"),
        },
    ] {
        assert!(
            crate::caddy_cli::run_offline(&registry, ServiceScope::User, command, &mut Vec::new(),)
                .await
                .is_err()
        );
    }
}
