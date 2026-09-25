use super::*;

#[test]
fn one_unit_per_scope_runs_the_manifest_daemon() {
    let binary = Path::new("/usr/local/bin/ocd");
    let user = render_systemd_unit(ServiceScope::User, None, binary).unwrap();
    let system = render_systemd_unit(ServiceScope::System, Some("operator"), binary).unwrap();
    assert!(user.contains("ExecStart=/usr/local/bin/ocd run\n"));
    assert!(system.contains("User=operator\nExecStart=/usr/local/bin/ocd --system run\n"));
    assert!(!user.contains("--config"));
    assert!(!system.contains("--config"));
    let user = render_launchd_plist(ServiceScope::User, None, binary).unwrap();
    let system = render_launchd_plist(ServiceScope::System, Some("operator"), binary).unwrap();
    assert!(user.contains("<string>dev.open-compute.ocd</string>"));
    assert!(system.contains("<string>--system</string>"));
    assert!(!user.contains("--config"));
    assert!(!system.contains("--config"));
}

#[test]
fn system_unit_rejects_missing_or_root_runtime_user() {
    for user in [None, Some(""), Some("root")] {
        assert!(render_systemd_unit(ServiceScope::System, user, Path::new("/opt/ocd")).is_err());
        assert!(render_launchd_plist(ServiceScope::System, user, Path::new("/opt/ocd")).is_err());
    }
}

#[test]
fn service_definitions_escape_paths_and_refuse_unsafe_replacement() {
    let path = Path::new("/opt/O'Compute & <test>/ocd");
    let systemd = render_systemd_unit(ServiceScope::User, None, path).unwrap();
    assert!(systemd.contains("'\\''"));
    let launchd = render_launchd_plist(ServiceScope::User, None, path).unwrap();
    assert!(launchd.contains("&amp; &lt;test&gt;"));
    let temp = tempfile::tempdir().unwrap();
    let definition = temp.path().join("unit");
    install_definition(&definition, b"first", "systemd unit").unwrap();
    install_definition(&definition, b"first", "systemd unit").unwrap();
    assert!(install_definition(&definition, b"changed", "systemd unit").is_err());
    assert_eq!(fs::read(&definition).unwrap(), b"first");
    assert!(install_definition(temp.path(), b"unit", "systemd unit").is_err());
    assert!(
        install_definition(&temp.path().join("missing/unit"), b"unit", "systemd unit").is_err()
    );
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, b"not a directory").unwrap();
    let manager = SystemdManager {
        unit_root: Some(blocked.join("units")),
    };
    assert_eq!(
        manager
            .install(ServiceScope::User, None, Path::new("/opt/ocd"))
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    assert_eq!(fs::read(&blocked).unwrap(), b"not a directory");
}

#[test]
fn user_service_definitions_follow_uid_home() {
    let home = crate::instance_registry::user_home_for_uid().unwrap();
    assert_eq!(
        SystemdManager::default()
            .unit_path(ServiceScope::User)
            .unwrap(),
        home.join(".config/systemd/user/dev.open-compute.ocd.service")
    );
    assert_eq!(
        LaunchdManager::default()
            .plist_path(ServiceScope::User)
            .unwrap(),
        home.join("Library/LaunchAgents/dev.open-compute.ocd.plist")
    );
}

#[test]
fn each_scope_installs_one_unit_even_with_multiple_instances() {
    let temp = tempfile::tempdir().unwrap();
    let systemd = SystemdManager {
        unit_root: Some(temp.path().join("systemd")),
    };
    systemd
        .install(ServiceScope::User, None, Path::new("/opt/ocd"))
        .unwrap();
    systemd
        .install(ServiceScope::User, None, Path::new("/opt/ocd"))
        .unwrap();
    assert_eq!(
        fs::read_dir(temp.path().join("systemd")).unwrap().count(),
        1
    );
    assert!(
        systemd
            .install(ServiceScope::User, None, Path::new("/different/ocd"))
            .is_err()
    );
    let launchd = LaunchdManager {
        plist_root: Some(temp.path().join("launchd")),
    };
    launchd
        .install(
            ServiceScope::System,
            Some("operator"),
            Path::new("/opt/ocd"),
        )
        .unwrap();
    launchd
        .install(
            ServiceScope::System,
            Some("operator"),
            Path::new("/opt/ocd"),
        )
        .unwrap();
    assert_eq!(
        fs::read_dir(temp.path().join("launchd")).unwrap().count(),
        1
    );
}

#[test]
fn fake_manager_tracks_scope_not_instance() {
    let fake = FakeServiceManager::default();
    fake.install(ServiceScope::User, None, Path::new("/opt/ocd"))
        .unwrap();
    fake.start(ServiceScope::User).unwrap();
    fake.start(ServiceScope::User).unwrap();
    assert_eq!(fake.installed(), vec![ServiceScope::User]);
    assert_eq!(fake.started(), vec![ServiceScope::User]);
    fake.stop(ServiceScope::User).unwrap();
    assert!(!fake.is_active(ServiceScope::User).unwrap());
    fake.uninstall(ServiceScope::User).unwrap();
    assert!(fake.installed().is_empty());
}

#[test]
fn scoped_managers_drive_only_their_own_definition() {
    let temp = tempfile::tempdir().unwrap();
    let binary = Path::new("/opt/ocd");
    let systemd = SystemdManager {
        unit_root: Some(temp.path().join("systemd")),
    };
    let launchd = LaunchdManager {
        plist_root: Some(temp.path().join("launchd")),
    };
    for scope in [ServiceScope::User, ServiceScope::System] {
        let user = matches!(scope, ServiceScope::System).then_some("operator");
        systemd.install(scope, user, binary).unwrap();
        launchd.install(scope, user, binary).unwrap();
        for manager in [&systemd as &dyn ServiceManager, &launchd] {
            manager.enable(scope).unwrap();
            manager.start(scope).unwrap();
            manager.restart(scope).unwrap();
            manager.stop(scope).unwrap();
            assert!(!manager.is_active(scope).unwrap());
        }
        systemd.uninstall(scope).unwrap();
        launchd.uninstall(scope).unwrap();
        assert!(!systemd.unit_path(scope).unwrap().exists());
        assert!(!launchd.plist_path(scope).unwrap().exists());
        systemd.uninstall(scope).unwrap();
        launchd.uninstall(scope).unwrap();
    }
    assert_eq!(
        fs::read_dir(temp.path().join("systemd")).unwrap().count(),
        0
    );
    assert_eq!(
        fs::read_dir(temp.path().join("launchd")).unwrap().count(),
        0
    );
    assert_eq!(
        launchd.logs(ServiceScope::User, false).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
}

#[test]
fn scoped_managers_report_command_results_without_touching_host_services() {
    if std::env::var_os("OPEN_COMPUTE_FAKE_MANAGER_CHILD").is_some() {
        let systemd = SystemdManager::default();
        if std::env::var_os("OPEN_COMPUTE_FAKE_MANAGER_FAIL").is_some() {
            assert!(systemd.enable(ServiceScope::User).is_err());
            assert!(systemd.start(ServiceScope::System).is_err());
            assert!(systemd.stop(ServiceScope::User).is_err());
            assert!(systemd.restart(ServiceScope::System).is_err());
            assert!(systemd.is_active(ServiceScope::User).is_err());
            assert!(systemd.logs(ServiceScope::User, false).is_err());
            let launchd = LaunchdManager::default();
            assert!(launchd.enable(ServiceScope::User).is_err());
            assert!(launchd.start(ServiceScope::User).is_err());
            assert!(launchd.stop(ServiceScope::User).is_err());
            assert!(launchd.restart(ServiceScope::User).is_err());
            assert!(!launchd.is_active(ServiceScope::User).unwrap());
            return;
        }
        if std::env::var_os("OPEN_COMPUTE_FAKE_MANAGER_INACTIVE").is_some() {
            assert!(!systemd.is_active(ServiceScope::User).unwrap());
            let launchd = LaunchdManager::default();
            assert!(!launchd.is_active(ServiceScope::User).unwrap());
            launchd.enable(ServiceScope::User).unwrap();
            launchd.start(ServiceScope::User).unwrap();
            return;
        }
        for scope in [ServiceScope::User, ServiceScope::System] {
            systemd.enable(scope).unwrap();
            systemd.start(scope).unwrap();
            systemd.restart(scope).unwrap();
            systemd.stop(scope).unwrap();
            assert!(systemd.is_active(scope).unwrap());
            assert!(systemd.logs(scope, false).unwrap().contains("fixture log"));
            assert_eq!(
                systemd.logs(scope, true).unwrap_err().code(),
                ErrorCode::PlatformUnavailable
            );
        }
        let launchd = LaunchdManager::default();
        for scope in [ServiceScope::User, ServiceScope::System] {
            launchd.enable(scope).unwrap();
            launchd.start(scope).unwrap();
            launchd.restart(scope).unwrap();
            launchd.stop(scope).unwrap();
            assert!(launchd.is_active(scope).unwrap());
        }
        return;
    }

    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    for name in ["systemctl", "launchctl", "journalctl"] {
        let path = temp.path().join(name);
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$OPEN_COMPUTE_FAKE_MANAGER_FAIL\" = 1 ]; then exit 1; fi\nif [ \"$OPEN_COMPUTE_FAKE_MANAGER_INACTIVE\" = 1 ]; then\n  case \"$*\" in *is-active*) exit 3;; *print*) exit 1;; esac\nfi\ncase \"$*\" in\n  *is-active*) printf 'active\\n';;\n  *print*) printf 'state = running\\n';;\n  *) printf 'fixture log\\n';;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "service_manager::tests::scoped_managers_report_command_results_without_touching_host_services"])
        .env("OPEN_COMPUTE_FAKE_MANAGER_CHILD", "1")
        .env("PATH", temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    let failed = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "service_manager::tests::scoped_managers_report_command_results_without_touching_host_services"])
        .env("OPEN_COMPUTE_FAKE_MANAGER_CHILD", "1")
        .env("OPEN_COMPUTE_FAKE_MANAGER_FAIL", "1")
        .env("PATH", temp.path())
        .status()
        .unwrap();
    assert!(failed.success());
    let inactive = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "service_manager::tests::scoped_managers_report_command_results_without_touching_host_services"])
        .env("OPEN_COMPUTE_FAKE_MANAGER_CHILD", "1")
        .env("OPEN_COMPUTE_FAKE_MANAGER_INACTIVE", "1")
        .env("PATH", temp.path())
        .status()
        .unwrap();
    assert!(inactive.success());
}
