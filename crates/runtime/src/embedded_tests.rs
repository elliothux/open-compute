//! Focused regressions for the embedded payload and its private materialization.

use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

#[test]
fn embedded_identity_is_pure_and_matches_the_formal_pin() {
    let (lock, bytes) = embedded_runtime_lock().unwrap();
    assert_eq!(RuntimeLock::parse(bytes).unwrap(), lock);
    assert_eq!(embedded_payload_sha256().len(), 64);
    assert_eq!(embedded_runtime_assets_sha256().len(), 64);
    let dir = tempfile::tempdir().unwrap();
    assert!(!inspect_embedded_runtime(&dir.path().join("absent")).unwrap());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn materialization_extracts_once_and_reuses_package() {
    let dir = tempfile::tempdir().unwrap();
    let package = materialize_embedded_runtime(dir.path()).unwrap();
    let executable = package.root.join("workerd");
    let pyodide = package
        .pyodide_bundle_cache_dir()
        .join(&embedded_runtime_lock().unwrap().0.pyodide_bundle.file_name);
    let inode = std::fs::metadata(&executable).unwrap().ino();
    assert_eq!(
        std::fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        std::fs::metadata(&pyodide).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert!(inspect_embedded_runtime(dir.path()).unwrap());
    assert!(open_materialized_runtime(dir.path()).is_ok());
    let runtime = package
        .verify(
            Duration::from_secs(20),
            &Redactor::new(),
            &dir.path().join("child.lease"),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.version_output(),
        embedded_runtime_lock().unwrap().0.expected_version_output
    );
    assert_eq!(
        runtime.pyodide_bundle_cache_dir(),
        Some(package.pyodide_bundle_cache_dir().as_path())
    );
    let again = materialize_embedded_runtime(dir.path()).unwrap();
    assert_eq!(package.root, again.root);
    assert_eq!(std::fs::metadata(&executable).unwrap().ino(), inode);
    assert_eq!(
        std::fs::read_dir(dir.path().join("packages"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn interrupted_materialization_cleanup_is_bounded_and_does_not_follow_links() {
    let dir = tempfile::tempdir().unwrap();
    let staging = dir
        .path()
        .join(format!(".partial-runtime-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("workerd"), b"incomplete").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let retained = outside.path().join("retained");
    std::fs::write(&retained, b"outside").unwrap();
    symlink(outside.path(), staging.join("link")).unwrap();
    cleanup_partial_packages(dir.path()).unwrap();
    assert!(!staging.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), b"outside");
}

#[test]
fn interrupted_compile_cleanup_removes_only_owned_staging() {
    let dir = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let compile = dir
        .path()
        .join(format!(".compile.{digest}.{}", uuid::Uuid::now_v7()));
    let partial = dir
        .path()
        .join(format!(".partial.{digest}.{}", uuid::Uuid::now_v7()));
    let atomic = dir
        .path()
        .join(format!(".partial.{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&compile).unwrap();
    std::fs::create_dir(&partial).unwrap();
    std::fs::write(compile.join("config.capnp"), b"incomplete").unwrap();
    std::fs::write(partial.join("config.bin"), b"incomplete").unwrap();
    std::fs::write(&atomic, b"incomplete").unwrap();
    let retained = dir.path().join(".partial.not-owned");
    std::fs::write(&retained, b"retained").unwrap();

    cleanup_interrupted_compile_state(dir.path()).unwrap();

    assert!(!compile.exists());
    assert!(!partial.exists());
    assert!(!atomic.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), b"retained");
}

#[test]
fn unpack_rejects_invalid_archives_and_oversized_payloads() {
    let dir = tempfile::tempdir().unwrap();
    assert!(unpack_payload(b"not gzip", &dir.path().join("bad-gzip"), 1024, Mode::RUSR,).is_err());
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(b"not the pinned executable").unwrap();
    let archive = encoder.finish().unwrap();
    assert!(unpack_payload(&archive, &dir.path().join("oversized"), 1, Mode::RUSR,).is_err());
}
