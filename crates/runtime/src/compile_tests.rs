use super::*;
use std::os::unix::fs::PermissionsExt;

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[tokio::test]
async fn compiled_cache_private_helpers_cover_success_cleanup_and_wait_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let digest = "ab".repeat(32);
    let path = tmp.path().join(format!("config.{digest}.bin"));
    let bytes = b"compiled config";

    assert!(try_reuse_or_clear_cache(&path, &digest).unwrap().is_none());
    let compiled = CompiledConfig::from_bytes_for_test(tmp.path(), &digest, bytes).unwrap();
    let expected = hex_sha256(&Sha256::digest(bytes).into());
    assert_eq!(validate_cache_entry(&path, &digest).unwrap(), expected);
    assert_eq!(wait_reuse_winner(&path, &digest).await.unwrap(), expected);
    assert_eq!(compiled.read_bytes().unwrap(), bytes);

    let forged = CompiledConfig {
        digest: digest.clone(),
        path: path.clone(),
        content_sha256: "00".repeat(32),
    };
    assert_eq!(
        forged.open().unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );

    fs::write(sidecar_path(&path), b"corrupt").unwrap();
    fs::set_permissions(sidecar_path(&path), fs::Permissions::from_mode(FILE_MODE)).unwrap();
    assert!(try_reuse_or_clear_cache(&path, &digest).unwrap().is_none());
    assert!(!path.exists());
    assert!(!sidecar_path(&path).exists());

    let missing = tmp.path().join("missing.bin");
    assert_eq!(
        wait_reuse_winner(&missing, &digest)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn compiled_cache_validation_rejects_every_file_and_sidecar_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let digest = "cd".repeat(32);
    let content = b"binary config";
    let content_hash = hex_sha256(&Sha256::digest(content).into());

    let directory = tmp.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(open_and_hash_compiled(&directory).is_err());

    let empty = tmp.path().join("empty.bin");
    write_mode(&empty, b"", FILE_MODE);
    assert_eq!(
        open_and_hash_compiled(&empty).unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
    write_mode(&empty, b"x", 0o644);
    assert_eq!(
        open_and_hash_compiled(&empty).unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );

    let oversized = tmp.path().join("oversized.bin");
    let file = File::create(&oversized).unwrap();
    file.set_len((MAX_COMPILED_BYTES + 1) as u64).unwrap();
    fs::set_permissions(&oversized, fs::Permissions::from_mode(FILE_MODE)).unwrap();
    assert_eq!(
        open_and_hash_compiled(&oversized).unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );

    let path = tmp.path().join("valid.bin");
    write_mode(&path, content, FILE_MODE);
    let sidecar = sidecar_path(&path);
    fs::create_dir(&sidecar).unwrap();
    assert!(validate_sidecar_for(&path, &digest, &content_hash).is_err());
    fs::remove_dir(&sidecar).unwrap();

    for (body, mode) in [
        (Vec::new(), FILE_MODE),
        (vec![b'x'; 257], FILE_MODE),
        (vec![0xff, 0xfe], FILE_MODE),
        (
            format!("{}\n{}\n", "ef".repeat(32), content_hash).into_bytes(),
            FILE_MODE,
        ),
        (
            format!("{digest}\n{}\n", "00".repeat(32)).into_bytes(),
            FILE_MODE,
        ),
        (format!("{digest}\n{content_hash}\n").into_bytes(), 0o644),
    ] {
        write_mode(&sidecar, &body, mode);
        assert_eq!(
            validate_sidecar_for(&path, &digest, &content_hash)
                .unwrap_err()
                .code(),
            ErrorCode::CacheEntryCorrupt
        );
        fs::remove_file(&sidecar).unwrap();
    }

    write_sidecar(&path, &digest, content).unwrap();
    assert!(validate_sidecar_for(&path, &digest, &content_hash).is_ok());
    assert_eq!(sidecar_path(&path).extension().unwrap(), "digest");
}
