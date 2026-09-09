use super::*;

#[tokio::test]
async fn default_doctor_does_not_mutate() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let mut perms = fs::metadata(&data).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&data, perms).unwrap();
    let before = snapshot(dir.path());
    let wal = data.join("control.sqlite-wal");
    let shm = data.join("control.sqlite-shm");
    assert!(!wal.exists());
    let loaded = load_fixture_platform_config(&path);
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert!(!wal.exists());
    assert!(!shm.exists());
    let after = snapshot(dir.path());
    assert_eq!(before, after);
    assert!(report.checks.iter().any(|c| c.name == "data_dir"));
    let human = {
        let mut buf = Vec::new();
        report.write(&mut buf, false).unwrap();
        String::from_utf8(buf).unwrap()
    };
    let json = {
        let mut buf = Vec::new();
        report.write(&mut buf, true).unwrap();
        String::from_utf8(buf).unwrap()
    };
    assert!(!human.contains("AKIA"));
    assert!(json.contains("schema_version"));
}
