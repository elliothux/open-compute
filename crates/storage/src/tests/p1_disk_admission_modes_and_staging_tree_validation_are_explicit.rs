use super::*;

#[test]
fn p1_disk_admission_modes_and_staging_tree_validation_are_explicit() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let hardening = HardeningConfig::default();
    let admission = crate::DiskAdmission::new(&config, &hardening);
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Serving
    );
    assert_eq!(
        admission.reserve(storage.data_dir(), 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    drop(admission.reserve(storage.data_dir(), 4096).unwrap());
    admission.begin_draining();
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Draining
    );
    let offline = crate::DiskAdmission::offline(&config, &hardening);
    assert_eq!(
        offline.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Offline
    );

    let nested = storage.data_dir().backup_staging_dir().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("payload"), b"12345").unwrap();
    assert!(
        admission
            .snapshot(storage.data_dir())
            .unwrap()
            .owned_staging_bytes
            >= 5
    );
    let link = nested.join("escape");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap_err().code(),
        ErrorCode::StoragePressure
    );
}
