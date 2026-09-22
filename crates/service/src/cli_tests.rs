use super::*;
use crate::exit::ExitClass;
use crate::instance_registry::{InstanceRegistry, ServiceScope};
use crate::service_manager::FakeServiceManager;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::SystemTime;
use tempfile::TempDir;

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
[server]
public_bind = "127.0.0.1:0"
admin_auth = {{ file = "{admin}" }}
deployer_auth = {{ file = "{deployer}" }}
read_only_auth = {{ file = "{read_only}" }}

[data]
path = "{data}"
master_key_file = "{master}"

[storage]
backend = "local"
path = "{objects}"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
        admin = admin.display(),
        deployer = deployer.display(),
        read_only = read_only.display(),
        data = data.display(),
        master = master.display(),
        objects = objects.display(),
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
            matches!(
                c,
                Command::Setup {
                    yes: true,
                    system: false
                }
            )
        }),
        (&["ocd", "setup", "--system", "--yes"], |c| {
            matches!(
                c,
                Command::Setup {
                    yes: true,
                    system: true
                }
            )
        }),
        (&["ocd", "instances"], |c| {
            matches!(c, Command::Instances { json: false })
        }),
        (
            &["ocd", "instance", "unregister", "--instance", "k7m2r"],
            |c| {
                matches!(
                    c,
                    Command::Instance {
                        command: InstanceCommand::Unregister { .. }
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
                "--account-id",
                "0123456789abcdef0123456789abcdef",
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
        (&["ocd", "wrangler", "--instance", "abcde", "deploy"], |c| {
            matches!(
                c,
                Command::Wrangler { arguments, .. }
                    if arguments == &["deploy"]
            )
        }),
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
            "abcde",
            "deploy",
        ])
        .is_err()
    );
}

#[test]
fn write_instances_empty_and_listed() {
    let temp = TempDir::new().unwrap();
    let deps = test_deps(&temp);
    let mut out = Vec::new();
    crate::instance_ops::write_instances(
        &deps.registry,
        deps.manager.as_ref(),
        None,
        &mut out,
        false,
    )
    .unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("No registered instances")
    );

    let config = write_loadable_config(temp.path());
    deps.registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let mut out = Vec::new();
    crate::instance_ops::write_instances(
        &deps.registry,
        deps.manager.as_ref(),
        None,
        &mut out,
        false,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("ID  STATE"));
    assert!(text.contains("stopped"));

    let mut out = Vec::new();
    crate::instance_ops::write_instances(
        &deps.registry,
        deps.manager.as_ref(),
        None,
        &mut out,
        true,
    )
    .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["command"], "instances");
    assert_eq!(payload["instances"].as_array().unwrap().len(), 1);
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
        ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
        ingress_ipv6: Vec::new(),
        https_listen: "127.0.0.1:8443".parse().unwrap(),
        challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
        proxy_protocol_from: Vec::new(),
        caddy: Vec::new(),
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
    let selector: InstanceSelector = "abcde".parse().unwrap();
    let err = resolve_loaded_config(
        Some(Path::new("/tmp/x.toml")),
        Some(&selector),
        temp.path(),
        Some(&deps.registry),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
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
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("INSTANCE_STARTED")
    );

    let listed = deps.registry.list().unwrap();
    assert_eq!(listed.len(), 1);
    let id = listed[0].instance_id.clone();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "--instance",
            &id,
            "status",
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
    assert_eq!(payload["command"], "status");

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
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(String::from_utf8(stdout).unwrap().contains("fake logs"));

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
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("INSTANCE_RESTARTED")
    );

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
    assert_eq!(code, ExitCode::from(ExitClass::Ok.code()));
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("INSTANCE_STOPPED")
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from([
            "ocd",
            "--no-update-check",
            "instance",
            "unregister",
            "--instance",
            &id,
        ])
        .unwrap(),
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
            .contains("INSTANCE_UNREGISTERED")
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
            "k7m2r",
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
            .contains("does not accept --instance")
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = execute_with_deps(
        parse_from(["ocd", "--no-update-check", "--instance", "k7m2r", "run"]).unwrap(),
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
            .contains("does not accept --instance")
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
    deps.registry
        .register(
            &config.canonicalize().unwrap(),
            ServiceScope::User,
            SystemTime::now(),
        )
        .unwrap();
    let id = deps.registry.list().unwrap()[0].instance_id.clone();
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
            .contains("cannot be combined")
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
        &parse_from(["ocd", "--instance", "abcde", "doctor"]).unwrap()
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
        parse_from(["ocd", "purge", "--instance", "abcde", "--dry-run"])
            .unwrap()
            .command,
        Command::Purge {
            yes: false,
            dry_run: true
        }
    ));
    assert!(parse_from(["ocd", "instance", "remove", "--instance", "abcde"]).is_err());
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
async fn execute_setup_yes_user_scope_with_config() {
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
    assert!(
        code == ExitCode::from(ExitClass::Ok.code())
            || !stderr.is_empty()
            || String::from_utf8_lossy(&stdout).contains("SETUP"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
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
        ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
        ingress_ipv6: vec!["2001:4860:4860::8888".parse().unwrap()],
        https_listen: "127.0.0.1:8443".parse().unwrap(),
        challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
        proxy_protocol_from: Vec::new(),
        caddy: Vec::new(),
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
    let token = temp.path().join("remote.token");
    write_mode(&token, "remote-secret\n", 0o600);
    let token = token.to_str().unwrap();
    let account = "0123456789abcdef0123456789abcdef";

    for args in [
        vec![
            "ocd",
            "--no-update-check",
            "target",
            "add",
            "remote",
            "--api-base-url",
            "https://remote.example/client/v4",
            "--account-id",
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
ingress_ipv4 = ["203.0.113.10"]
ingress_ipv6 = []
https_listen = "127.0.0.1:8443"
challenge_dns_listen = "127.0.0.1:8053"
proxy_protocol_from = []
"#,
        )
        .unwrap();
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
        let error = run_loaded(Command::Config { command }, loaded, &mut Vec::new())
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
ingress_ipv4 = ["203.0.113.10"]
https_listen = "127.0.0.1:8443"
challenge_dns_listen = "127.0.0.1:8053"
"#
    )
    .unwrap();
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    drop(DataDir::acquire(&loaded.config.data).unwrap());

    let mut modules = Vec::new();
    run_loaded(
        Command::Caddy {
            command: CaddyCommand::ListModules,
        },
        loaded.clone(),
        &mut modules,
    )
    .await
    .unwrap();
    let modules = String::from_utf8(modules).unwrap();
    assert!(modules.contains("dns.providers.opencompute"));

    let source = temp.path().join("site.caddyfile");
    fs::write(&source, "example.com { respond ok }").unwrap();
    let mut formatted = Vec::new();
    run_loaded(
        Command::Caddy {
            command: CaddyCommand::Fmt {
                file: source.clone(),
            },
        },
        loaded.clone(),
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
    run_loaded(
        Command::Caddy {
            command: CaddyCommand::Validate,
        },
        loaded,
        &mut validated,
    )
    .await
    .unwrap();
    assert_eq!(validated, b"CADDY_CONFIG_OK\n");
}

#[tokio::test]
async fn online_caddy_tools_use_the_instance_control_socket() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixListener;

    let temp = TempDir::new().unwrap();
    let config = write_loadable_config(temp.path());
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    let data = DataDir::acquire(&loaded.config.data).unwrap();
    open_compute_runtime::materialize_embedded_runtime(&data.runtime_dir()).unwrap();
    drop(data);
    let id = open_compute_core::InstanceId::from_canonical_config_path(&loaded.path).unwrap();
    let runtime = crate::instance_control::runtime_dir_for(ServiceScope::User, &id, None);
    fs::create_dir_all(&runtime).unwrap();
    let socket = runtime.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            assert!(request.contains("caddy_"));
            writeln!(stream, r#"{{"schema_version":1,"ok":true,"gateway_status":{{"schema_version":1,"child_pid":41,"tls_ready":true,"config_sha256":"abc","last_reload":"ok","last_error":null,"dns":"ok"}}}}"#).unwrap();
        }
    });

    let mut modules = Vec::new();
    run_loaded(
        Command::Caddy {
            command: CaddyCommand::ListModules,
        },
        loaded.clone(),
        &mut modules,
    )
    .await
    .unwrap();
    assert!(
        String::from_utf8(modules)
            .unwrap()
            .contains("dns.providers.opencompute")
    );

    for command in [
        CaddyCommand::Status,
        CaddyCommand::Reload,
        CaddyCommand::Validate,
    ] {
        let mut output = Vec::new();
        run_loaded(Command::Caddy { command }, loaded.clone(), &mut output)
            .await
            .unwrap();
        assert!(!output.is_empty());
    }
    server.join().unwrap();
    fs::remove_file(socket).unwrap();
    fs::remove_dir(runtime).unwrap();
}

#[tokio::test]
async fn caddy_tools_fail_closed_without_required_inputs() {
    let temp = TempDir::new().unwrap();
    let config = write_loadable_config(temp.path());
    let loaded = load_platform_config_from(&config, temp.path()).unwrap();
    drop(DataDir::acquire(&loaded.config.data).unwrap());

    for command in [
        CaddyCommand::Status,
        CaddyCommand::Validate,
        CaddyCommand::Fmt {
            file: temp.path().join("missing.caddyfile"),
        },
    ] {
        assert!(
            run_loaded(Command::Caddy { command }, loaded.clone(), &mut Vec::new())
                .await
                .is_err()
        );
    }
}
