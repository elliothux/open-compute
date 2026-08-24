use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::symlink;

#[test]
fn archive_and_decompressed_binary_have_independent_bounds() {
    assert_eq!(
        checked_decompressed_len(64 * 1024 * 1024, 1).unwrap(),
        64 * 1024 * 1024 + 1
    );
    assert!(checked_decompressed_len(MAX_WORKERD_BINARY_BYTES, 1).is_err());
    assert!(checked_decompressed_len(usize::MAX, 1).is_err());
}

#[test]
fn operator_fetch_and_copy_helpers_fail_closed_on_invalid_sources() {
    let target = RuntimeTarget {
        archive_name: "workerd-test.gz".to_owned(),
        archive_url: "https://127.0.0.1:9/workerd-test.gz".to_owned(),
        archive_sha256: "00".repeat(32),
        binary_sha256: "00".repeat(32),
    };
    assert_eq!(
        download_official(&target).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    let dir = tempfile::TempDir::new().unwrap();
    let source_dir = dir.path().join("source");
    let destination = dir.path().join("destination");
    fs::create_dir(&source_dir).unwrap();
    assert_eq!(
        copy_regular(&source_dir, &destination, 0o644)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let ordinary_file = dir.path().join("ordinary");
    fs::write(&ordinary_file, b"not a directory").unwrap();
    assert_eq!(
        copy_tree(dir.path(), &ordinary_file, &destination)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let tree = dir.path().join("tree");
    fs::create_dir(&tree).unwrap();
    let socket_path = tree.join("socket");
    let _socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    assert_eq!(
        copy_tree(dir.path(), &tree, &dir.path().join("tree-copy"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        parent_of_dest(Path::new("/")).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    let missing_tree = dir.path().join("missing-tree");
    assert_eq!(
        copy_tree(dir.path(), &missing_tree, &dir.path().join("missing-copy"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let nested = dir.path().join("nested-source");
    fs::create_dir_all(nested.join("child")).unwrap();
    fs::write(nested.join("child/file"), b"nested").unwrap();
    let nested_dest = dir.path().join("nested-dest");
    copy_tree(dir.path(), &nested, &nested_dest).unwrap();
    assert_eq!(fs::read(nested_dest.join("child/file")).unwrap(), b"nested");

    let linked = dir.path().join("linked-source");
    fs::create_dir(&linked).unwrap();
    symlink(&ordinary_file, linked.join("link")).unwrap();
    assert_eq!(
        copy_tree(dir.path(), &linked, &dir.path().join("linked-copy"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

fn current_target() -> (&'static str, &'static str) {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("darwin-arm64", "workerd-darwin-arm64.gz"),
        ("macos", "x86_64") => ("darwin-x64", "workerd-darwin-64.gz"),
        ("linux", "x86_64") => ("linux-x64", "workerd-linux-64.gz"),
        ("linux", "aarch64") => ("linux-arm64", "workerd-linux-arm64.gz"),
        other => panic!("unsupported test target {other:?}"),
    }
}

fn install_lock(binary: &[u8], archive: &[u8]) -> RuntimeLock {
    let (target_name, archive_name) = current_target();
    let release = "v1.test";
    RuntimeLock {
        schema_version: 1,
        release: release.to_owned(),
        expected_version_output: "never completes".to_owned(),
        host_compatibility_date: "2026-08-22".to_owned(),
        process_flags: vec!["--experimental".to_owned()],
        host_compatibility_flags: vec!["nodejs_compat".to_owned()],
        targets: BTreeMap::from([(
            target_name.to_owned(),
            RuntimeTarget {
                archive_name: archive_name.to_owned(),
                archive_url: format!(
                    "https://github.com/cloudflare/workerd/releases/download/{release}/{archive_name}"
                ),
                archive_sha256: hex::encode(Sha256::digest(archive)),
                binary_sha256: hex::encode(Sha256::digest(binary)),
            },
        )]),
    }
}

#[test]
fn installer_bounds_archives_and_reaps_a_version_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let oversized = vec![0_u8; MAX_ARCHIVE_BYTES + 1];
    let dummy_lock = install_lock(b"dummy", b"archive");
    assert_eq!(
        install_official_release(
            &dummy_lock,
            &dir.path().join("oversized"),
            false,
            Some(&oversized),
        )
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );

    let binary = b"#!/bin/sh\nsleep 30\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(binary).unwrap();
    let archive = encoder.finish().unwrap();
    let lock = install_lock(binary, &archive);
    let destination = dir.path().join("timeout");
    let started = std::time::Instant::now();
    assert_eq!(
        install_official_release(&lock, &destination, false, Some(&archive))
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );
    assert!(started.elapsed() >= VERSION_TIMEOUT);
    assert!(!destination.exists());
}
