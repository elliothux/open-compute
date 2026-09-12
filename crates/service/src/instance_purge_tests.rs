use super::*;
use crate::instance_registry::ServiceScope;
use crate::service_manager::FakeServiceManager;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::time::SystemTime;
use tempfile::TempDir;

fn write_config(root: &Path, data: &Path, objects: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::create_dir_all(data).unwrap();
    fs::create_dir_all(objects).unwrap();
    fs::set_permissions(data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(objects, fs::Permissions::from_mode(0o700)).unwrap();
    let admin = root.join("admin.token");
    let deployer = root.join("deployer.token");
    let read_only = root.join("read-only.token");
    for path in [&admin, &deployer, &read_only] {
        fs::write(path, b"secret-value\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = root.join("compute.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_auth = {{ file = "{}" }}
deployer_auth = {{ file = "{}" }}
read_only_auth = {{ file = "{}" }}

[data]
path = "{}"
master_key_file = "{}"

[storage]
backend = "local"
path = "{}"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
            admin.display(),
            deployer.display(),
            read_only.display(),
            data.display(),
            data.join("keys/master.key").display(),
            objects.display(),
        ),
    )
    .unwrap();
    config
}

fn fixture() -> (
    TempDir,
    InstanceRegistry,
    InstanceRecord,
    PathBuf,
    PathBuf,
    PathBuf,
) {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let data = base.join("data");
    let objects = base.join("external-objects");
    let config = write_config(&base.join("config"), &data, &objects);
    let binary = base.join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let registry =
        InstanceRegistry::with_roots(base.join("registry/system"), base.join("registry/user"));
    let record = registry
        .register_owned(
            &config.canonicalize().unwrap(),
            &binary,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
    (temp, registry, record, config, data, objects)
}

fn initialize_owned_external_authority(config: &Path) {
    let loaded = load_platform_config_from(config, Path::new("/")).unwrap();
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let connected =
        crate::object_storage::connect_object_backend(&loaded.config, storage.identity()).unwrap();
    storage
        .bind_object_authority(
            connected.backend.kind(),
            &connected.backend.authority_sha256(),
        )
        .unwrap();
}

fn plan_from(record: &InstanceRecord) -> PurgePlan {
    let (external_local_root, external_authority) = match &record.object_authority {
        RegisteredObjectAuthority::Local { path } => (Some(PathBuf::from(path)), None),
        RegisteredObjectAuthority::S3 { endpoint, bucket } => (
            None,
            Some(format!("s3 endpoint={endpoint} bucket={bucket}")),
        ),
    };
    PurgePlan {
        record: record.clone(),
        config_path: record.config_path().to_owned(),
        config_sha256: record.config_sha256.clone(),
        data_dir: PathBuf::from(&record.data_path),
        external_local_root,
        retained_local_root: None,
        external_authority,
    }
}

fn write_s3_config(root: &Path, data: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    fs::create_dir_all(data).unwrap();
    fs::set_permissions(data, fs::Permissions::from_mode(0o700)).unwrap();
    let config = root.join("compute.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_auth = {{ env = "PURGE_ADMIN" }}
deployer_auth = {{ env = "PURGE_DEPLOYER" }}
read_only_auth = {{ env = "PURGE_READ_ONLY" }}

[data]
path = "{}"
master_key_file = "{}"

[storage]
backend = "s3"
endpoint = "https://s3.example.test"
region = "auto"
bucket = "purge-fixture"
force_path_style = true
access_key_id_env = "PURGE_ACCESS_KEY"
secret_access_key_env = "PURGE_SECRET_KEY"
prefix = "system/"

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536
"#,
            data.display(),
            data.join("keys/master.key").display(),
        ),
    )
    .unwrap();
    config
}

#[derive(Debug)]
enum ManagerAction {
    FailStatus,
    ChangeConfig(PathBuf),
    ReplaceConfigWithSymlink(PathBuf),
    CorruptDataTree(PathBuf),
    RemoveRegistration(InstanceRegistry, InstanceSelector),
}

#[derive(Debug)]
struct LifecycleManager(ManagerAction);

impl ServiceManager for LifecycleManager {
    fn install(&self, _: &InstanceRecord, _: &Path) -> Result<(), PlatformError> {
        Ok(())
    }

    fn enable(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn start(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn stop(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn restart(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn uninstall(&self, _: &InstanceRecord) -> Result<(), PlatformError> {
        match &self.0 {
            ManagerAction::ChangeConfig(path) => fs::write(path, b"changed after planning")
                .map_err(|_| PlatformError::new(ErrorCode::Internal, "test mutation failed")),
            ManagerAction::ReplaceConfigWithSymlink(path) => {
                let target = path.with_extension("target");
                fs::rename(path, &target)
                    .and_then(|()| std::os::unix::fs::symlink(&target, path))
                    .map_err(|_| PlatformError::new(ErrorCode::Internal, "test mutation failed"))
            }
            ManagerAction::CorruptDataTree(path) => {
                std::os::unix::fs::symlink("missing", path.join("late-link"))
                    .map_err(|_| PlatformError::new(ErrorCode::Internal, "test mutation failed"))
            }
            ManagerAction::RemoveRegistration(registry, selector) => {
                registry.remove(selector).map(|_| ())
            }
            ManagerAction::FailStatus => Ok(()),
        }
    }

    fn is_active(&self, _: &InstanceRecord) -> Result<bool, PlatformError> {
        if matches!(self.0, ManagerAction::FailStatus) {
            Err(PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "test status failure",
            ))
        } else {
            Ok(false)
        }
    }

    fn logs(&self, _: &InstanceRecord, _: bool) -> Result<String, PlatformError> {
        Ok(String::new())
    }
}

#[derive(Debug)]
struct FailWriter;

impl Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("test output failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct NeedleFailWriter {
    needle: &'static str,
    bytes: Vec<u8>,
}

impl NeedleFailWriter {
    fn new(needle: &'static str) -> Self {
        Self {
            needle,
            bytes: Vec::new(),
        }
    }
}

impl Write for NeedleFailWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if String::from_utf8_lossy(bytes).contains(self.needle) {
            return Err(io::Error::other("selected test output failure"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailReader;

impl io::Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("test input failure"))
    }
}

impl BufRead for FailReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Err(io::Error::other("test input failure"))
    }

    fn consume(&mut self, _: usize) {}
}

#[test]
fn dry_run_emits_exact_plan_without_mutation() {
    let (_temp, registry, record, config, data, objects) = fixture();
    let mut out = Vec::new();
    purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        false,
        true,
        &mut out,
    )
    .unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("PURGE_PLAN"));
    assert!(output.contains(config.to_string_lossy().as_ref()));
    assert!(output.contains(data.to_string_lossy().as_ref()));
    assert!(output.contains(objects.to_string_lossy().as_ref()));
    assert!(config.exists() && data.exists() && objects.exists());
    assert_eq!(registry.list().unwrap().len(), 1);
    if !io::stdin().is_terminal() {
        let error = purge_records(
            std::slice::from_ref(&record),
            &registry,
            &FakeServiceManager::default(),
            None,
            false,
            false,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        assert!(config.exists() && data.exists() && objects.exists());
        assert_eq!(registry.list().unwrap().len(), 1);
    }
}

#[test]
fn purge_stops_unregisters_and_deletes_only_resolved_local_state() {
    let (temp, registry, record, config, data, objects) = fixture();
    initialize_owned_external_authority(&config);
    fs::write(objects.join("object"), b"bytes").unwrap();
    let manager = FakeServiceManager::default();
    manager.install(&record, record.binary_path()).unwrap();
    manager.start(&record).unwrap();
    let mut out = Vec::new();
    purge_records(
        std::slice::from_ref(&record),
        &registry,
        &manager,
        Some(&temp.path().join("runtime")),
        true,
        false,
        &mut out,
    )
    .unwrap();
    assert!(!config.exists() && !data.exists() && !objects.exists());
    assert!(registry.list().unwrap().is_empty());
    assert!(!manager.is_active(&record).unwrap());
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("INSTANCE_UNREGISTERED"));
    assert!(output.contains("PURGE_OK instances=1"));
}

#[test]
fn purge_retains_external_local_root_without_matching_authority_identity() {
    let (_temp, registry, record, config, data, objects) = fixture();
    fs::write(objects.join("unrelated"), b"retain").unwrap();
    let mut out = Vec::new();
    purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut out,
    )
    .unwrap();
    assert!(!config.exists() && !data.exists());
    assert!(objects.join("unrelated").exists());
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("ownership=unproven")
    );
}

#[test]
fn purge_refuses_changed_config_before_mutation() {
    let (_temp, registry, record, config, data, objects) = fixture();
    let mut body = fs::read_to_string(&config).unwrap();
    body.push_str("\n# changed\n");
    fs::write(&config, body).unwrap();
    let err = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(config.exists() && data.exists() && objects.exists());
    assert_eq!(registry.list().unwrap().len(), 1);
}

#[test]
fn purge_refuses_symlink_anywhere_in_delete_tree() {
    let (temp, registry, record, config, data, objects) = fixture();
    std::os::unix::fs::symlink(temp.path().join("outside"), data.join("link")).unwrap();
    let err = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(config.exists() && data.exists() && objects.exists());
}

#[test]
fn selection_owned_filter_and_preserving_unregister_cover_exact_entry_points() {
    let (temp, registry, record, config, data, objects) = fixture();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    assert_eq!(
        owned_records(&registry, record.binary_path())
            .unwrap()
            .len(),
        1
    );
    assert!(
        owned_records(&registry, Path::new("/different/ocd"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        select_record(None, None, temp.path(), &registry)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        select_record(Some(&config), Some(&selector), temp.path(), &registry)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        select_record(None, Some(&selector), temp.path(), &registry).unwrap(),
        record
    );
    assert_eq!(
        select_record(Some(&config), None, temp.path(), &registry).unwrap(),
        record
    );
    let other = write_config(
        &temp.path().join("other-config"),
        &temp.path().join("other-data"),
        &temp.path().join("other-objects"),
    );
    assert_eq!(
        select_record(Some(&other), None, temp.path(), &registry)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceNotFound
    );
    assert_eq!(
        run_selected_purge(
            None,
            Some(&selector),
            temp.path(),
            &registry,
            &FakeServiceManager::default(),
            None,
            true,
            true,
            &mut Vec::new(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::InstanceRegistryInvalid
    );

    let mut output = Vec::new();
    unregister_preserving_data(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        &mut output,
    )
    .unwrap();
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("UNINSTALL_RETAIN")
    );
    unregister_preserving_data(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        false,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert!(config.exists() && data.exists() && objects.exists());
}

#[test]
fn selected_purge_accepts_current_binary_and_s3_state_is_retained() {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let data = base.join("data");
    let config = write_s3_config(&base.join("config"), &data);
    let registry =
        InstanceRegistry::with_roots(base.join("registry/system"), base.join("registry/user"));
    let current = std::env::current_exe().unwrap().canonicalize().unwrap();
    let record = registry
        .register_owned(
            &config.canonicalize().unwrap(),
            &current,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let mut out = Vec::new();
    run_selected_purge(
        None,
        Some(&selector),
        &base,
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        true,
        &mut out,
    )
    .unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("PURGE_DRY_RUN_OK"));
    assert!(output.contains("s3 endpoint=https://s3.example.test"));
    assert!(output.contains("bucket=purge-fixture"));

    let loaded = load_platform_config_from(&config, Path::new("/")).unwrap();
    assert!(!local_authority_is_uniquely_owned(&loaded.config));
    let mut out = Vec::new();
    purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut out,
    )
    .unwrap();
    assert!(!data.exists() && !config.exists());
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("delete external objects manually")
    );
}

#[test]
fn nested_local_authority_and_lifecycle_plan_text_are_complete() {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let data = base.join("data");
    let objects = data.join("objects");
    let config = write_config(&base.join("config"), &data, &objects);
    let binary = base.join("bin/ocd");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, b"ocd").unwrap();
    let registry =
        InstanceRegistry::with_roots(base.join("registry/system"), base.join("registry/user"));
    let record = registry
        .register_owned(
            &config.canonicalize().unwrap(),
            &binary,
            ServiceScope::User,
            None,
            SystemTime::now(),
        )
        .unwrap();
    let plans = build_plans(std::slice::from_ref(&record), &registry, true).unwrap();
    assert!(plans[0].external_local_root.is_none());
    assert!(plans[0].retained_local_root.is_none());
    let mut all_fields = plan_from(&record);
    all_fields.retained_local_root = Some(base.join("retained"));
    all_fields.external_authority = Some("s3 endpoint=https://example.test bucket=b".to_owned());
    let mut out = Vec::new();
    write_plan(&all_fields, "PLAN", &mut out).unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains("PLAN_LOCAL_OBJECTS"));
    assert!(output.contains("PLAN_EXTERNAL_RETAINED local"));
    assert!(output.contains("PLAN_EXTERNAL_RETAINED s3"));
    assert_eq!(
        write_plan(&all_fields, "PLAN", &mut FailWriter)
            .unwrap_err()
            .code(),
        ErrorCode::Internal
    );
    assert_eq!(io_failed().code(), ErrorCode::Internal);
}

#[test]
fn purge_path_validation_rejects_ambiguous_targets_and_overlaps() {
    let (temp, _registry, record, config, data, objects) = fixture();
    let base = temp.path().canonicalize().unwrap();
    let mut plan = plan_from(&record);
    plan.retained_local_root = Some(PathBuf::from("/tmp/control\npath"));
    assert_eq!(
        validate_printable_plan(&plan).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    plan.retained_local_root = None;
    plan.external_authority = Some("s3 bucket=bad\nname".to_owned());
    assert_eq!(
        validate_printable_plan(&plan).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    for unsafe_path in [
        PathBuf::from("relative"),
        PathBuf::from("/"),
        base.join("parent/../child"),
    ] {
        let mut unsafe_plan = plan_from(&record);
        unsafe_plan.data_dir = unsafe_path;
        assert_eq!(
            validate_plan_paths(&unsafe_plan).unwrap_err().code(),
            ErrorCode::PathInvalid
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut home_plan = plan_from(&record);
        home_plan.data_dir = PathBuf::from(home);
        assert_eq!(
            validate_plan_paths(&home_plan).unwrap_err().code(),
            ErrorCode::PathInvalid
        );
    }

    let target = base.join("symlink-target");
    fs::create_dir(&target).unwrap();
    let link = base.join("symlink-root");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut direct_link = plan_from(&record);
    direct_link.data_dir = link.clone();
    assert_eq!(
        validate_plan_paths(&direct_link).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let mut ancestor_link = plan_from(&record);
    ancestor_link.data_dir = link.join("child");
    assert_eq!(
        validate_plan_paths(&ancestor_link).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    let hardlink = base.join("config-hardlink");
    fs::hard_link(&config, &hardlink).unwrap();
    assert_eq!(
        validate_plan_paths(&plan_from(&record)).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    fs::remove_file(hardlink).unwrap();
    let mut directory_config = plan_from(&record);
    directory_config.config_path = data.clone();
    assert_eq!(
        validate_plan_paths(&directory_config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let mut overlapping_config = plan_from(&record);
    overlapping_config.config_path = data.join("future-config");
    assert_eq!(
        validate_plan_paths(&overlapping_config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    let mut nested = plan_from(&record);
    nested.record.instance_id.push('x');
    nested.data_dir = data.join("nested");
    assert_eq!(
        validate_no_overlaps(&[plan_from(&record), nested], &[])
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let mut protected = record.clone();
    protected.instance_id.push('y');
    protected.data_path = objects.to_string_lossy().into_owned();
    assert_eq!(
        validate_no_overlaps(&[plan_from(&record)], std::slice::from_ref(&protected))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    protected.data_path = base.join("protected-data").to_string_lossy().into_owned();
    protected.object_authority = RegisteredObjectAuthority::Local {
        path: data
            .join("protected-objects")
            .to_string_lossy()
            .into_owned(),
    };
    assert_eq!(
        validate_no_overlaps(&[plan_from(&record)], &[protected])
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let mut s3_protected = record.clone();
    s3_protected.instance_id.push('z');
    s3_protected.data_path = base.join("s3-data").to_string_lossy().into_owned();
    s3_protected.object_authority = RegisteredObjectAuthority::S3 {
        endpoint: "https://s3.example.test".to_owned(),
        bucket: "protected".to_owned(),
    };
    assert!(validate_no_overlaps(&[plan_from(&record)], &[s3_protected]).is_ok());
    assert!(overlaps(&data, &data.join("nested")));
    assert!(!overlaps(&data, &base.join("separate")));
}

#[test]
fn purge_tree_config_confirmation_and_remaining_helpers_cover_fail_closed_edges() {
    let (temp, _registry, record, config, data, objects) = fixture();
    let base = temp.path().canonicalize().unwrap();
    assert!(validate_delete_tree(&base.join("missing")).is_ok());
    let file = base.join("plain-file");
    fs::write(&file, b"x").unwrap();
    assert_eq!(
        validate_delete_tree(&file).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let valid_tree = base.join("valid-tree/nested");
    fs::create_dir_all(&valid_tree).unwrap();
    fs::write(valid_tree.join("file"), b"x").unwrap();
    validate_delete_tree(&base.join("valid-tree")).unwrap();
    let linked = base.join("linked-tree");
    fs::create_dir(&linked).unwrap();
    let first = linked.join("first");
    fs::write(&first, b"x").unwrap();
    fs::hard_link(&first, linked.join("second")).unwrap();
    assert_eq!(
        validate_delete_tree(&linked).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    let plan = plan_from(&record);
    confirm(&[], false, false, &mut io::empty(), &mut Vec::new()).unwrap();
    confirm(
        std::slice::from_ref(&plan),
        true,
        false,
        &mut io::empty(),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
        confirm(
            std::slice::from_ref(&plan),
            false,
            false,
            &mut io::empty(),
            &mut Vec::new(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigInvalid
    );
    let mut plain_plan = plan.clone();
    plain_plan.external_local_root = None;
    plain_plan.retained_local_root = None;
    plain_plan.external_authority = None;
    let plain_confirmation = format!(
        "purge instance={} config={} data={}",
        plain_plan.record.instance_id,
        plain_plan.config_path.display(),
        plain_plan.data_dir.display()
    );
    confirm(
        std::slice::from_ref(&plain_plan),
        false,
        true,
        &mut io::Cursor::new(format!("{plain_confirmation}\n")),
        &mut Vec::new(),
    )
    .unwrap();
    let mut confirmation_plan = plan.clone();
    confirmation_plan.external_local_root = Some(objects.clone());
    confirmation_plan.retained_local_root = Some(base.join("retained-local"));
    confirmation_plan.external_authority =
        Some("s3 endpoint=https://example.test bucket=b".to_owned());
    let expected = format!(
        "purge instance={} config={} data={} local_objects={} retained_local_objects={} retained=s3 endpoint=https://example.test bucket=b",
        confirmation_plan.record.instance_id,
        confirmation_plan.config_path.display(),
        confirmation_plan.data_dir.display(),
        objects.display(),
        base.join("retained-local").display()
    );
    let mut prompt = Vec::new();
    confirm(
        std::slice::from_ref(&confirmation_plan),
        false,
        true,
        &mut io::Cursor::new(format!("{expected}\n")),
        &mut prompt,
    )
    .unwrap();
    assert!(String::from_utf8(prompt).unwrap().contains(&expected));
    assert_eq!(
        confirm(
            std::slice::from_ref(&confirmation_plan),
            false,
            true,
            &mut io::Cursor::new("wrong\n"),
            &mut Vec::new(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        confirm(
            std::slice::from_ref(&confirmation_plan),
            false,
            true,
            &mut io::Cursor::new(format!("{expected}\n")),
            &mut FailWriter,
        )
        .unwrap_err()
        .code(),
        ErrorCode::Internal
    );
    assert_eq!(
        confirm(
            std::slice::from_ref(&confirmation_plan),
            false,
            true,
            &mut FailReader,
            &mut Vec::new(),
        )
        .unwrap_err()
        .code(),
        ErrorCode::ConfigInvalid
    );
    verify_config_unchanged(&plan).unwrap();
    let mut wrong_digest = plan.clone();
    wrong_digest.config_sha256 = "00".repeat(32);
    assert_eq!(
        verify_config_unchanged(&wrong_digest).unwrap_err().code(),
        ErrorCode::InstanceRegistryInvalid
    );
    let mut missing_config = plan.clone();
    missing_config.config_path = base.join("missing-config");
    assert_eq!(
        verify_config_unchanged(&missing_config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let mut out = Vec::new();
    delete_config(&missing_config, &mut out).unwrap();
    let mut directory = plan.clone();
    directory.config_path = base.join("config-directory");
    fs::create_dir(&directory.config_path).unwrap();
    assert_eq!(
        delete_config(&directory, &mut Vec::new())
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert!(remove_tree(&base.join("absent-tree")).is_ok());
    assert_eq!(
        remove_tree(&file).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let removable = base.join("removable");
    fs::create_dir(&removable).unwrap();
    remove_tree(&removable).unwrap();

    let blocked = base.join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        validate_no_symlink_ancestors(&blocked.join("child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();

    let mut remaining = plan;
    remaining.external_local_root = Some(objects.clone());
    remaining.retained_local_root = Some(base.join("retained-local"));
    remaining.external_authority = Some("s3 endpoint=https://example.test bucket=b".to_owned());
    let mut out = Vec::new();
    write_remaining(std::slice::from_ref(&remaining), &mut out).unwrap();
    let output = String::from_utf8(out).unwrap();
    assert!(output.contains(config.to_string_lossy().as_ref()));
    assert!(output.contains(data.to_string_lossy().as_ref()));
    assert!(output.contains(objects.to_string_lossy().as_ref()));
    assert!(output.contains("retained-local"));
    assert!(output.contains("s3 endpoint="));
    let mut retained_only = remaining;
    retained_only.config_path = base.join("missing-config");
    retained_only.data_dir = base.join("missing-data");
    retained_only.external_local_root = None;
    retained_only.external_authority = None;
    assert_eq!(
        write_remaining(&[retained_only], &mut FailWriter)
            .unwrap_err()
            .code(),
        ErrorCode::Internal
    );
}

#[test]
fn purge_reports_service_config_tree_and_registry_failures_with_retryable_state() {
    let (_temp, registry, record, config, data, _objects) = fixture();
    let mut out = Vec::new();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &LifecycleManager(ManagerAction::FailStatus),
        None,
        true,
        false,
        &mut out,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PlatformUnavailable);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("PURGE_INSTANCE_FAILED")
    );
    assert!(config.exists() && data.exists());

    let (_temp, registry, record, config, data, _objects) = fixture();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &LifecycleManager(ManagerAction::ChangeConfig(config.clone())),
        None,
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceRegistryInvalid);
    assert!(config.exists() && data.exists());

    let (_temp, registry, record, config, data, _objects) = fixture();
    let mut out = Vec::new();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &LifecycleManager(ManagerAction::CorruptDataTree(data.clone())),
        None,
        true,
        false,
        &mut out,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(String::from_utf8(out).unwrap().contains("PURGE_REMAINS"));
    assert!(config.exists() && data.exists());

    let (_temp, registry, record, config, data, _objects) = fixture();
    let selector: InstanceSelector = record.instance_id.parse().unwrap();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &LifecycleManager(ManagerAction::RemoveRegistration(
            registry.clone(),
            selector,
        )),
        None,
        true,
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceNotFound);
    assert!(config.exists());
    assert!(!data.exists());

    let (_temp, registry, record, config, data, _objects) = fixture();
    let mut out = Vec::new();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &LifecycleManager(ManagerAction::ReplaceConfigWithSymlink(config.clone())),
        None,
        true,
        false,
        &mut out,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(
        fs::symlink_metadata(&config)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!data.exists());
    assert!(registry.list().unwrap().is_empty());
    assert!(String::from_utf8(out).unwrap().contains("PURGE_REMAINS"));
}

#[test]
fn purge_output_failures_preserve_the_retry_boundary() {
    let (_temp, registry, record, config, data, _objects) = fixture();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut NeedleFailWriter::new("PURGE_SERVICE_UNREGISTERED"),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(config.exists() && data.exists());
    assert_eq!(registry.list().unwrap().len(), 1);

    let (_temp, registry, record, config, data, _objects) = fixture();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut NeedleFailWriter::new("INSTANCE_UNREGISTERED"),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(config.exists());
    assert!(!data.exists());
    assert!(registry.list().unwrap().is_empty());

    let (_temp, registry, record, config, data, _objects) = fixture();
    let error = purge_records(
        std::slice::from_ref(&record),
        &registry,
        &FakeServiceManager::default(),
        None,
        true,
        false,
        &mut NeedleFailWriter::new("PURGE_OK"),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(!config.exists() && !data.exists());
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn malformed_selected_record_and_unproven_authority_fail_closed() {
    let (temp, registry, record, config, data, objects) = fixture();
    let loaded = load_platform_config_from(&config, Path::new("/")).unwrap();
    fs::remove_dir_all(&objects).unwrap();
    assert!(!local_authority_is_uniquely_owned(&loaded.config));
    fs::create_dir(&objects).unwrap();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o700)).unwrap();
    initialize_owned_external_authority(&config);
    fs::remove_dir_all(&objects).unwrap();
    fs::write(&objects, b"not a local object authority").unwrap();
    assert!(!local_authority_is_uniquely_owned(&loaded.config));

    let mut malformed = record.clone();
    malformed.instance_id = "not valid".to_owned();
    let error = unregister_preserving_data(
        std::slice::from_ref(&malformed),
        &registry,
        &FakeServiceManager::default(),
        Some(&temp.path().join("runtime")),
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InstanceIdInvalid);
    assert!(config.exists() && data.exists());
    assert_eq!(registry.list().unwrap().len(), 1);
}
