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
async fn materialization_is_reused_verified_and_never_repairs_corruption() {
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

    let asset = package.assets_dir().join("config.capnp");
    std::fs::set_permissions(&asset, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&asset, b"corrupt").unwrap();
    let token = open_compute_core::SecretString::new("a".repeat(64));
    let binding_token = open_compute_core::SecretString::new("b".repeat(64));
    let observability_token = open_compute_core::SecretString::new("c".repeat(64));
    let compile_error = crate::compile_static_config(crate::CompileRequest {
        runtime: &runtime,
        lock_path: &package.lock_path(),
        assets_dir: &package.assets_dir(),
        runtime_data_dir: dir.path(),
        platform: &crate::PlatformReleaseMeta {
            version: "embedded-test".to_owned(),
        },
        token: &token,
        binding_token: &binding_token,
        observability_token: &observability_token,
        durable_objects: open_compute_core::DurableObjectsConfig::default(),
        deadline: Duration::from_secs(20),
        redactor: &Redactor::new(),
    })
    .await
    .unwrap_err();
    assert_eq!(compile_error.code(), ErrorCode::RuntimeInvalid);
    assert!(materialize_embedded_runtime(dir.path()).is_err());
    assert!(inspect_embedded_runtime(dir.path()).is_err());
    assert_eq!(std::fs::read(&asset).unwrap(), b"corrupt");
}

#[test]
fn materialization_never_repairs_a_corrupt_pyodide_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let package = materialize_embedded_runtime(dir.path()).unwrap();
    let lock = embedded_runtime_lock().unwrap().0;
    let bundle = package
        .pyodide_bundle_cache_dir()
        .join(&lock.pyodide_bundle.file_name);
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&bundle).unwrap())),
        lock.pyodide_bundle.bundle_sha256
    );
    std::fs::set_permissions(&bundle, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&bundle, b"corrupt").unwrap();
    assert!(materialize_embedded_runtime(dir.path()).is_err());
    assert!(inspect_embedded_runtime(dir.path()).is_err());
    assert_eq!(std::fs::read(&bundle).unwrap(), b"corrupt");
}

#[test]
fn materialization_rejects_symlink_roots_and_existing_partial_packages() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    symlink(other.path(), dir.path().join("packages")).unwrap();
    assert!(materialize_embedded_runtime(dir.path()).is_err());
    assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    let fresh = tempfile::tempdir().unwrap();
    let root = fresh
        .path()
        .join("packages")
        .join(embedded_payload_sha256());
    std::fs::create_dir_all(&root).unwrap();
    assert!(materialize_embedded_runtime(fresh.path()).is_err());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert!(materialize_embedded_runtime(Path::new("relative")).is_err());
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
    symlink(outside.path(), &staging).unwrap();
    assert!(cleanup_partial_packages(dir.path()).is_err());
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

    let outside = tempfile::tempdir().unwrap();
    let linked = dir
        .path()
        .join(format!(".partial.{digest}.{}", uuid::Uuid::now_v7()));
    symlink(outside.path(), &linked).unwrap();
    assert!(cleanup_interrupted_compile_state(dir.path()).is_err());
}

#[test]
fn unpack_rejects_invalid_archives_and_wrong_payload_hashes() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        unpack_payload(
            b"not gzip",
            &dir.path().join("bad-gzip"),
            &"0".repeat(64),
            1024,
            Mode::RUSR,
        )
        .is_err()
    );
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(b"not the pinned executable").unwrap();
    let archive = encoder.finish().unwrap();
    assert!(
        unpack_payload(
            &archive,
            &dir.path().join("bad-hash"),
            &"0".repeat(64),
            1024,
            Mode::RUSR,
        )
        .is_err()
    );
    assert!(
        unpack_payload(
            &archive,
            &dir.path().join("oversized"),
            &"0".repeat(64),
            1,
            Mode::RUSR,
        )
        .is_err()
    );
}
