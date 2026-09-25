use super::*;

#[tokio::test]
async fn doctor_rejects_future_schema_and_sha256_symlink_and_corrupt_cache() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let data = dir.path().join("data");
    let db = data.join("control.sqlite");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO refinery_schema_history(version,name,applied_on,checksum)
             VALUES(?1,'future','1970-01-01T00:00:00Z','0')",
            [open_compute_storage::migrations::current_schema_version() + 1],
        )
        .unwrap();
    }
    let loaded = load_fixture_platform_config(&path);
    let report = doctor_report(&loaded, DoctorMode::Basic, None).await;
    assert_eq!(check(&report, "sqlite").status, CheckStatus::Failed);

    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let cache = dir.path().join("data/cache/artifacts");
    let sha = cache.join("sha256");
    let _ = fs::remove_dir_all(&sha);
    std::os::unix::fs::symlink("/tmp", &sha).unwrap();
    let loaded = load_fixture_platform_config(&path);
    let report = doctor_report(&loaded, DoctorMode::Basic, None).await;
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Failed
    );

    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let digest = "ab".repeat(32);
    let shard = dir
        .path()
        .join("data/cache/artifacts/sha256")
        .join(&digest[..2]);
    fs::create_dir_all(&shard).unwrap();
    let entry = shard.join(&digest[2..]);
    fs::write(&entry, b"corrupt-bytes").unwrap();
    let before_meta = fs::symlink_metadata(&entry).unwrap();
    let before_bytes = fs::read(&entry).unwrap();
    let loaded = load_fixture_platform_config(&path);
    let report = doctor_report(&loaded, DoctorMode::Basic, None).await;
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Failed
    );
    assert_eq!(fs::read(&entry).unwrap(), before_bytes);
    let after = fs::symlink_metadata(&entry).unwrap();
    assert_eq!(after.len(), before_meta.len());
    assert_eq!(after.modified().ok(), before_meta.modified().ok());
}
