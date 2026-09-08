use super::*;

#[tokio::test]
async fn opened_file_sources_require_exact_private_single_link_files() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/source/value").unwrap();
    let path = fixture.config.path.parent().unwrap().join("source");
    fs::write(&path, b"source").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mismatched = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: mismatched,
                    length: 5,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    let link = path.with_extension("link");
    fs::hard_link(&path, &link).unwrap();
    let linked = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: linked,
                    length: 6,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::remove_file(link).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let public = OpenOptions::new().read(true).open(&path).unwrap();
    assert_eq!(
        fixture
            .backend
            .put(
                &key,
                ObjectSource::File {
                    file: public,
                    length: 6,
                },
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
}
