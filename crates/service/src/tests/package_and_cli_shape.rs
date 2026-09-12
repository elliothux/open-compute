use super::*;

#[test]
fn package_and_cli_shape() {
    assert_eq!(env!("CARGO_PKG_NAME"), "open-compute-service");
    let help = Cli::command().render_help().to_string();
    assert!(help.contains("ocd"));
    assert!(help.contains("run"));
    assert!(help.contains("config"));
    assert!(help.contains("doctor"));
    assert!(help.contains("scheduler"));
    assert!(help.contains("capabilities"));
    assert!(help.contains("backup"));
    assert!(help.contains("instances"));
    assert!(help.contains("start"));
    assert!(help.contains("stop"));
    assert!(help.contains("status"));
    assert!(help.contains("no-update-check"));
    assert!(!help.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed == "__update_check" || trimmed.starts_with("__update_check ")
    }));
    assert!(help.contains("upgrade"));
    assert!(help.contains("uninstall"));
    assert!(help.contains("support-bundle"));
    let parsed = parse_from(["ocd", "run", "--config", "/tmp/config.toml"]).unwrap();
    assert!(matches!(parsed.command, Command::Run));
    assert!(
        parse_from([
            "ocd",
            "--config",
            "/tmp/a.toml",
            "--instance",
            "k7m2r",
            "instances"
        ])
        .is_err()
    );
    assert!(parse_from(["ocd", "run", "--instance", "k7m2r"]).is_ok());
    let parsed = parse_from(["ocd", "instances", "--json"]).unwrap();
    assert!(matches!(parsed.command, Command::Instances { json: true }));
    assert!(!parsed.no_update_check);
    let parsed = parse_from(["ocd", "--no-update-check", "instances"]).unwrap();
    assert!(parsed.no_update_check);
    let parsed = parse_from(["ocd", "upgrade", "--dry-run"]).unwrap();
    assert!(matches!(
        parsed.command,
        Command::Upgrade {
            dry_run: true,
            no_restart: false,
            version: None
        }
    ));
    let parsed = parse_from(["ocd", "upgrade", "0.1.1", "--no-restart"]).unwrap();
    assert!(matches!(
        parsed.command,
        Command::Upgrade {
            dry_run: false,
            no_restart: true,
            version: Some(ref version)
        } if version == "0.1.1"
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
        parse_from(["ocd", "__update_check"]).unwrap().command,
        Command::UpdateCheck
    ));
    let parsed = parse_from([
        "ocd",
        "config",
        "check",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Config {
            command: ConfigCommand::Check { json: true }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "create",
        "--name",
        "before-snapshot",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::Create { json: true, .. }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "cleanup-restore",
        "--staging",
        "01900000-0000-7000-8000-000000000000",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::CleanupRestore { json: true, .. }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "attest-restore-smoke",
        "--snapshot",
        "01900000-0000-7000-8000-000000000000",
        "--passed",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::AttestRestoreSmoke {
                passed: true,
                json: true,
                ..
            }
        }
    ));
    assert!(parse_from(["ocd", "upgrade"]).is_ok());
    let parsed = parse_from([
        "ocd",
        "doctor",
        "--config",
        "/tmp/config.toml",
        "--full",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Doctor {
            full: true,
            json: true
        }
    ));
    let parsed = parse_from([
        "ocd",
        "scheduler",
        "recover-corrupt",
        "--backup-name",
        "scheduler-corrupt-test",
        "--config",
        "/tmp/config.toml",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Scheduler {
            command: SchedulerCommand::RecoverCorrupt { .. }
        }
    ));
}
