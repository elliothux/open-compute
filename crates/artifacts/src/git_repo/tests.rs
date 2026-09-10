use super::*;
use std::process::Command;

fn run(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn store_with_limit(max_object_bytes: u64) -> (tempfile::TempDir, GitRepositoryStore) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("artifacts");
    let quarantine = temp.path().join("quarantine");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&quarantine).unwrap();
    set_private_permissions(&root, true).unwrap();
    set_private_permissions(&quarantine, true).unwrap();
    let store = GitRepositoryStore::open(root, quarantine, max_object_bytes).unwrap();
    (temp, store)
}

#[test]
fn initialize_push_read_fork_and_source_delete_preserve_objects() {
    let (temp, store) = store_with_limit(1024 * 1024);
    let source = ArtifactRepoId::generate();
    let target = ArtifactRepoId::generate();
    store.initialize(source, "main").unwrap();

    let work = temp.path().join("work");
    run(
        temp.path(),
        &[
            "clone",
            store.path(source).to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    run(&work, &["config", "user.name", "Open Compute Test"]);
    run(&work, &["config", "user.email", "test@open-compute.dev"]);
    std::fs::write(work.join("README.md"), b"artifact content\n").unwrap();
    run(&work, &["add", "README.md"]);
    run(&work, &["commit", "-m", "first"]);
    run(&work, &["push", "origin", "main"]);

    let head = store.resolve_revision(source, "main").unwrap();
    let tree = run_output(&work, &["rev-parse", "HEAD^{tree}"]);
    let blob = run_output(&work, &["rev-parse", "HEAD:README.md"]);
    assert_eq!(head.len(), 40);
    assert_eq!(store.default_branch(source).unwrap(), "main");
    assert!(store.object_count(source).unwrap() >= 3);
    assert_eq!(
        store.read_object(source, &head).unwrap().kind,
        GitObjectKind::Commit
    );
    assert_eq!(
        store.read_object(source, &tree).unwrap().kind,
        GitObjectKind::Tree
    );
    assert_eq!(
        store.read_object(source, &blob).unwrap(),
        GitObject {
            oid: blob.clone(),
            kind: GitObjectKind::Blob,
            bytes: b"artifact content\n".to_vec(),
        }
    );
    assert_eq!(store.resolve_revision(source, &head).unwrap(), head);
    assert_eq!(
        store.resolve_revision(source, "refs/heads/main").unwrap(),
        head
    );
    run(&work, &["tag", "-a", "release", "-m", "release"]);
    run(&work, &["push", "origin", "refs/tags/release"]);
    let tag = run_output(&work, &["rev-parse", "release"]);
    assert_eq!(
        store.read_object(source, &tag).unwrap().kind,
        GitObjectKind::Tag
    );
    assert_eq!(store.resolve_revision(source, "release").unwrap(), head);
    store
        .validate_advertised_wants(source, std::slice::from_ref(&head))
        .unwrap();
    std::fs::write(work.join("hidden.txt"), b"unreachable\n").unwrap();
    run(&work, &["add", "hidden.txt"]);
    run(&work, &["commit", "-m", "hidden"]);
    run(&work, &["push", "origin", "HEAD:refs/heads/hidden"]);
    let hidden = store.resolve_revision(source, "hidden").unwrap();
    run(
        temp.path(),
        &[
            "--git-dir",
            store.path(source).to_str().unwrap(),
            "update-ref",
            "-d",
            "refs/heads/hidden",
        ],
    );
    assert_eq!(
        store
            .validate_advertised_wants(source, &[hidden])
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        store.read_file(source, "main", "README.md").unwrap().bytes,
        b"artifact content\n"
    );
    let log = store.commit_log(source, "main", 0, 2).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, GitObjectKind::Commit);
    assert!(store.repository_size(source, 16 * 1024 * 1024).unwrap() > 0);
    run(&work, &["switch", "-c", "feature"]);
    std::fs::write(work.join("feature.txt"), b"feature\n").unwrap();
    run(&work, &["add", "feature.txt"]);
    run(&work, &["commit", "-m", "feature"]);
    run(&work, &["push", "origin", "feature"]);
    store
        .fork(source, target, true, "main", 16 * 1024 * 1024)
        .unwrap();
    assert_eq!(
        store
            .resolve_revision(target, "feature")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    store.delete(source).unwrap();
    store.delete(source).unwrap();
    assert_eq!(
        store.read_file(target, "main", "README.md").unwrap().bytes,
        b"artifact content\n"
    );
}

fn run_output(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn import_rejects_local_private_and_credentialed_remotes_before_network() {
    let (_temp, store) = store_with_limit(1024 * 1024);
    for remote in [
        "file:///tmp/repo.git",
        "http://example.com/repo.git",
        "https://127.0.0.1/repo.git",
        "https://user:secret@example.com/repo.git",
        "https://example.com/repo.git?token=secret",
    ] {
        let error = store
            .import_public_https(
                ArtifactRepoId::generate(),
                remote,
                None,
                None,
                1024 * 1024,
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PathInvalid, "{remote}");
    }
}

#[test]
fn object_limit_and_unsafe_paths_fail_closed() {
    let (temp, store) = store_with_limit(1024);
    let source = ArtifactRepoId::generate();
    store.initialize(source, "main").unwrap();
    let work = temp.path().join("work");
    run(
        temp.path(),
        &[
            "clone",
            store.path(source).to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    run(&work, &["config", "user.name", "Open Compute Test"]);
    run(&work, &["config", "user.email", "test@open-compute.dev"]);
    std::fs::write(work.join("file.txt"), vec![b'x'; 2048]).unwrap();
    run(&work, &["add", "file.txt"]);
    run(&work, &["commit", "-m", "large"]);
    run(&work, &["push", "origin", "main"]);
    assert_eq!(
        store
            .read_file(source, "main", "../file.txt")
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        store
            .read_file(source, "main", "file.txt")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimitExceeded
    );
    assert_eq!(
        store
            .read_object(source, &run_output(&work, &["rev-parse", "HEAD:file.txt"]))
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimitExceeded
    );
    for (revision, offset, limit) in [("main", 0, 0), ("main", 0, 201), ("main", 100_001, 1)] {
        assert_eq!(
            store
                .commit_log(source, revision, offset, limit)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }
    for oid in ["bad", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"] {
        assert_eq!(
            store.read_object(source, oid).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
}

#[test]
fn repository_lifecycle_and_open_validation_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let quarantine = temp.path().join("quarantine");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&quarantine).unwrap();
    set_private_permissions(&root, true).unwrap();
    set_private_permissions(&quarantine, true).unwrap();
    assert!(GitRepositoryStore::open(root.clone(), root.clone(), 1).is_err());
    assert!(GitRepositoryStore::open(root.clone(), quarantine.clone(), 0).is_err());
    assert!(GitRepositoryStore::open(PathBuf::from("relative"), quarantine.clone(), 1).is_err());
    let store = GitRepositoryStore::open(root, quarantine.clone(), 1024 * 1024).unwrap();
    for branch in ["", "/main", "main/", "main.", "main..next", "main@next"] {
        assert_eq!(
            store
                .initialize(ArtifactRepoId::generate(), branch)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }

    let unpublished = ArtifactRepoId::generate();
    store.initialize(unpublished, "main").unwrap();
    assert_eq!(
        store.initialize(unpublished, "main").unwrap_err().code(),
        ErrorCode::ResourceNameConflict
    );
    store.discard_unpublished(unpublished).unwrap();
    store.discard_unpublished(unpublished).unwrap();

    let corrupt = ArtifactRepoId::generate();
    std::fs::write(store.path(corrupt), b"corrupt").unwrap();
    assert!(store.discard_unpublished(corrupt).is_err());
    store.quarantine_corrupt(corrupt).unwrap();
    assert!(quarantine.join(format!("{corrupt}.git")).is_file());
    store.cleanup_quarantine().unwrap();
    assert!(!quarantine.join(format!("{corrupt}.git")).exists());
    store.quarantine_corrupt(corrupt).unwrap();

    let quarantined_dir = ArtifactRepoId::generate();
    std::fs::create_dir(quarantine.join(format!("{quarantined_dir}.git"))).unwrap();
    store.cleanup_quarantine().unwrap();
    assert!(!quarantine.join(format!("{quarantined_dir}.git")).exists());
}
