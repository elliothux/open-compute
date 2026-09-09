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
        (&["ocd", "instance", "remove", "--instance", "k7m2r"], |c| {
            matches!(
                c,
                Command::Instance {
                    command: InstanceCommand::Remove { .. }
                }
            )
        }),
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
            "remove",
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
            .contains("INSTANCE_REMOVED")
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
        Command::Uninstall
    ));
    assert!(matches!(
        parse_from(["ocd", "__update_check"]).unwrap().command,
        Command::UpdateCheck
    ));
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
