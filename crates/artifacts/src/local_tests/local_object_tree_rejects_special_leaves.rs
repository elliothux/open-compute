use super::*;

#[tokio::test]
async fn local_object_tree_rejects_special_leaves() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/special/value").unwrap();
    let parent = fixture.object_file(&key).parent().unwrap().to_owned();
    fs::create_dir_all(&parent).unwrap();
    for ancestor in parent
        .ancestors()
        .take_while(|path| *path != fixture.config.path)
    {
        fs::set_permissions(ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(
        std::process::Command::new("mkfifo")
            .arg(fixture.object_file(&key))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
}
