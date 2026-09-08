use super::*;
use crate::instance_registry::REGISTRY_SCHEMA_VERSION;

fn sample_record() -> InstanceRecord {
    InstanceRecord {
        schema_version: REGISTRY_SCHEMA_VERSION,
        instance_id: "k7m2r".to_owned(),
        digest_sha256: "00".repeat(32),
        canonical_config_path: "/etc/open-compute/config.toml".to_owned(),
        service_scope: ServiceScope::User,
        service_user: None,
        service_identifier: "dev.open-compute.ocd.k7m2r".to_owned(),
        created_at: 0,
    }
}

#[test]
fn systemd_unit_embeds_absolute_ocd_and_config() {
    let unit = render_systemd_unit(&sample_record(), Path::new("/usr/local/bin/ocd")).unwrap();
    assert!(
        unit.contains("ExecStart=/usr/local/bin/ocd --config /etc/open-compute/config.toml run")
    );
    assert!(unit.contains("WantedBy=default.target"));
    assert!(!unit.contains("User="));
}

#[test]
fn launchd_plist_embeds_program_arguments() {
    let plist = render_launchd_plist(&sample_record(), Path::new("/usr/local/bin/ocd")).unwrap();
    assert!(plist.contains("<string>/usr/local/bin/ocd</string>"));
    assert!(plist.contains("<string>/etc/open-compute/config.toml</string>"));
    assert!(plist.contains("<string>run</string>"));
}

#[test]
fn fake_manager_tracks_lifecycle() {
    let fake = FakeServiceManager::default();
    let record = sample_record();
    fake.install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    fake.start(&record).unwrap();
    assert!(fake.is_active(&record).unwrap());
    fake.stop(&record).unwrap();
    assert!(!fake.is_active(&record).unwrap());
    fake.uninstall(&record).unwrap();
    assert!(fake.installed().is_empty());
}

#[test]
fn render_escapes_shell_and_xml_metacharacters() {
    let mut record = sample_record();
    record.canonical_config_path = "/tmp/weird'path & <x>.toml".to_owned();
    let unit = render_systemd_unit(&record, Path::new("/usr/local/bin/ocd")).unwrap();
    assert!(unit.contains("'/tmp/weird'\\''path & <x>.toml'"));
    let plist = render_launchd_plist(&record, Path::new("/tmp/ocd & <bin>")).unwrap();
    assert!(plist.contains("/tmp/ocd &amp; &lt;bin&gt;"));
    assert!(plist.contains("&amp;") && plist.contains("&lt;"));
}

#[test]
fn launchd_install_and_uninstall_use_plist_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = LaunchdManager {
        plist_root: Some(temp.path().to_path_buf()),
    };
    let record = sample_record();
    manager
        .install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    let path = temp
        .path()
        .join(format!("{}.plist", record.service_identifier));
    assert!(path.is_file());
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("dev.open-compute.ocd.k7m2r"));
    manager.enable(&record).unwrap();
    manager.start(&record).unwrap();
    assert!(!manager.is_active(&record).unwrap());
    manager.restart(&record).unwrap();
    assert!(manager.logs(&record, false).is_err());
    manager.uninstall(&record).unwrap();
    assert!(!path.exists());
}

#[test]
fn systemd_install_and_uninstall_use_unit_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = SystemdManager {
        unit_root: Some(temp.path().to_path_buf()),
    };
    let record = sample_record();
    manager
        .install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    let path = temp
        .path()
        .join(format!("{}.service", record.service_identifier));
    assert!(path.is_file());
    manager.enable(&record).unwrap();
    manager.start(&record).unwrap();
    manager.stop(&record).unwrap();
    manager.restart(&record).unwrap();
    assert!(!manager.is_active(&record).unwrap());
    assert!(manager.logs(&record, true).is_err());
    manager.uninstall(&record).unwrap();
    assert!(!path.exists());
}

#[test]
fn unsupported_manager_fails_closed() {
    let manager = UnsupportedManager;
    let record = sample_record();
    assert!(
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .is_err()
    );
    assert!(manager.enable(&record).is_err());
    assert!(manager.start(&record).is_err());
    assert!(manager.stop(&record).is_err());
    assert!(manager.restart(&record).is_err());
    assert!(manager.uninstall(&record).is_err());
    assert!(manager.is_active(&record).is_err());
    assert!(manager.logs(&record, false).is_err());
}

#[test]
fn host_service_manager_returns_platform_adapter() {
    let manager = host_service_manager();
    // Only exercise the constructor; OS command paths are not invoked here.
    let _ = manager.logs(&sample_record(), true);
}

#[test]
fn systemd_without_unit_root_fails_closed_on_missing_systemctl() {
    let manager = SystemdManager { unit_root: None };
    let mut record = sample_record();
    record.service_scope = ServiceScope::User;
    record.service_identifier = "dev.open-compute.ocd.coverage-miss".to_owned();
    // On hosts without systemd these invoke fail immediately; do not install units.
    assert!(manager.enable(&record).is_err());
    assert!(manager.start(&record).is_err());
    assert!(manager.stop(&record).is_err());
    assert!(manager.restart(&record).is_err());
    let _ = manager.is_active(&record);
    let _ = manager.logs(&record, false);
    assert!(manager.logs(&record, true).is_err());
}

#[test]
fn systemd_install_creates_nested_unit_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let nested = temp.path().join("nested/units");
    let manager = SystemdManager {
        unit_root: Some(nested.clone()),
    };
    let mut record = sample_record();
    record.service_scope = ServiceScope::System;
    record.service_user = Some("ocd-service".to_owned());
    manager
        .install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    assert!(
        nested
            .join(format!("{}.service", record.service_identifier))
            .is_file()
    );
    // Missing unit file uninstall is a no-op after remove.
    manager.uninstall(&record).unwrap();
    manager.uninstall(&record).unwrap();
}

#[test]
fn launchd_nested_plist_root_and_missing_uninstall() {
    let temp = tempfile::TempDir::new().unwrap();
    let nested = temp.path().join("agents/nested");
    let manager = LaunchdManager {
        plist_root: Some(nested.clone()),
    };
    let mut record = sample_record();
    record.service_scope = ServiceScope::System;
    record.service_user = Some("ocd-service".to_owned());
    manager
        .install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    assert!(
        nested
            .join(format!("{}.plist", record.service_identifier))
            .is_file()
    );
    manager.stop(&record).unwrap();
    manager.uninstall(&record).unwrap();
    manager.uninstall(&record).unwrap();
}

#[test]
fn fake_manager_restart_failure_and_logs() {
    let fake = FakeServiceManager::default();
    let record = sample_record();
    fake.install(&record, Path::new("/usr/local/bin/ocd"))
        .unwrap();
    fake.enable(&record).unwrap();
    fake.start(&record).unwrap();
    assert!(fake.logs(&record, false).unwrap().contains("fake logs"));
    fake.set_fail_restart(true);
    assert_eq!(
        fake.restart(&record).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
    fake.set_fail_restart(false);
    fake.restart(&record).unwrap();
    assert!(fake.is_active(&record).unwrap());
    assert_eq!(fake.started(), vec![record.service_identifier.clone()]);
}

#[test]
fn render_covers_system_scope_identifiers() {
    let mut record = sample_record();
    record.service_scope = ServiceScope::System;
    record.service_user = Some("ocd-service".to_owned());
    let unit = render_systemd_unit(&record, Path::new("/opt/ocd")).unwrap();
    assert!(unit.contains("WantedBy=multi-user.target"));
    assert!(unit.contains("User=ocd-service"));
    let plist = render_launchd_plist(&record, Path::new("/opt/ocd")).unwrap();
    assert!(plist.contains("<key>Label</key>"));
    assert!(plist.contains("<key>UserName</key>"));
    assert_eq!(
        unit_name(&record),
        format!("{}.service", record.service_identifier)
    );
    assert_eq!(launch_domain(&record), "system");
}

#[test]
fn launchd_without_plist_root_fails_closed_on_launchctl() {
    let manager = LaunchdManager { plist_root: None };
    let mut record = sample_record();
    record.service_identifier = "dev.open-compute.ocd.coverage-launchd".to_owned();
    // Real launchctl against a missing unit should fail closed quickly.
    assert!(manager.enable(&record).is_err());
    assert!(manager.start(&record).is_err());
    assert!(manager.stop(&record).is_err());
    let _ = manager.restart(&record);
    let _ = manager.is_active(&record);
    assert!(manager.logs(&record, false).is_err());
    assert!(manager.logs(&record, true).is_err());
}

#[test]
fn launch_domain_covers_user_gui_scope() {
    let record = sample_record();
    assert!(launch_domain(&record).starts_with("gui/"));
}

#[test]
fn systemd_unit_path_and_install_fail_closed() {
    let manager = SystemdManager { unit_root: None };
    let mut record = sample_record();
    record.service_scope = ServiceScope::System;
    let path = manager.unit_path(&record).unwrap();
    assert!(path.starts_with("/etc/systemd/system"));
    record.service_scope = ServiceScope::User;
    let path = manager.unit_path(&record).unwrap();
    assert!(path.to_string_lossy().contains(".config/systemd/user"));

    let temp = tempfile::TempDir::new().unwrap();
    let file_root = temp.path().join("not-a-dir");
    fs::write(&file_root, b"x").unwrap();
    let bad = SystemdManager {
        unit_root: Some(file_root),
    };
    assert!(
        bad.install(&sample_record(), Path::new("/usr/local/bin/ocd"))
            .is_err()
    );

    let ok_root = tempfile::TempDir::new().unwrap();
    let manager = SystemdManager {
        unit_root: Some(ok_root.path().to_path_buf()),
    };
    let record = sample_record();
    // Pre-create the unit path as a directory so atomic_write fails closed.
    let unit = manager.unit_path(&record).unwrap();
    fs::create_dir_all(&unit).unwrap();
    assert!(
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .is_err()
    );
}

#[test]
fn launchd_install_fails_when_plist_path_is_directory() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = LaunchdManager {
        plist_root: Some(temp.path().to_path_buf()),
    };
    let record = sample_record();
    let path = manager.plist_path(&record).unwrap();
    fs::create_dir_all(&path).unwrap();
    assert!(
        manager
            .install(&record, Path::new("/usr/local/bin/ocd"))
            .is_err()
    );
}

#[test]
fn service_definition_install_is_idempotent_but_never_replaces_content() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("service.unit");
    install_definition(&path, b"first", "custom").unwrap();
    install_definition(&path, b"first", "custom").unwrap();
    let err = install_definition(&path, b"second", "custom").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    assert_eq!(fs::read(&path).unwrap(), b"first");

    let directory = temp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    let err = install_definition(&directory, b"body", "custom").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
}

#[test]
fn system_service_definitions_require_an_account() {
    let mut record = sample_record();
    record.service_scope = ServiceScope::System;
    let unit_error = render_systemd_unit(&record, Path::new("/opt/ocd")).unwrap_err();
    assert_eq!(unit_error.code(), ErrorCode::InstanceRegistryInvalid);
    let plist_error = render_launchd_plist(&record, Path::new("/opt/ocd")).unwrap_err();
    assert_eq!(plist_error.code(), ErrorCode::InstanceRegistryInvalid);
}

#[test]
fn uninstall_propagates_definition_removal_errors() {
    let systemd_root = tempfile::TempDir::new().unwrap();
    let systemd = SystemdManager {
        unit_root: Some(systemd_root.path().to_path_buf()),
    };
    let record = sample_record();
    fs::create_dir(systemd.unit_path(&record).unwrap()).unwrap();
    assert_eq!(
        systemd.uninstall(&record).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );

    let launchd_root = tempfile::TempDir::new().unwrap();
    let launchd = LaunchdManager {
        plist_root: Some(launchd_root.path().to_path_buf()),
    };
    fs::create_dir(launchd.plist_path(&record).unwrap()).unwrap();
    assert_eq!(
        launchd.uninstall(&record).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
}
