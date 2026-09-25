use super::*;

#[tokio::test]
async fn local_fresh_host_restore_requires_complete_directory_backup() {
    let temp = TempDir::new().unwrap();
    let config = write_config(temp.path(), "");
    let loaded = load_fixture_platform_config(&config);
    let before = snapshot(temp.path());

    let error = crate::backup_cli::backup_restore(&loaded, "unused", &[])
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RestoreInvalid);
    assert_eq!(snapshot(temp.path()), before);
}
