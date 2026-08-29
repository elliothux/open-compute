//! P1 deterministic local reliability runner contract Gate.

use std::fs;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace")
        .to_path_buf()
}

#[test]
fn p1_local_runners_and_runbooks_are_complete_and_safe() {
    let root = workspace();
    for script in ["soak-p1.sh", "load-p1.sh", "fuzz-p1.sh"] {
        let source = fs::read_to_string(root.join("test").join(script)).expect("P1 runner");
        assert!(source.starts_with("#!/bin/sh\n"), "{script}");
        assert!(source.contains("set -eu"), "{script}");
        assert!(!source.contains("curl "), "{script} must stay local");
        assert!(!source.contains("sudo "), "{script} must stay unprivileged");
    }
    for runbook in [
        "install-and-first-start.md",
        "backup-and-retention.md",
        "fresh-host-restore.md",
        "current-release-recovery.md",
        "disk-pressure.md",
        "sqlite-corruption.md",
        "s3-outage.md",
        "workerd-crash-loop.md",
        "master-key-loss-and-recovery.md",
        "scheduler-recovery.md",
        "collect-support-bundle.md",
    ] {
        let source = fs::read_to_string(root.join("docs/references/runbooks").join(runbook))
            .expect("P1 runbook");
        for section in [
            "触发信号",
            "影响面",
            "只读诊断",
            "允许的 mutation",
            "预期",
            "停止条件",
            "回滚",
            "验证",
        ] {
            assert!(source.contains(section), "{runbook} missing {section}");
        }
        assert!(!source.contains("$HOME"), "{runbook}");
        assert!(!source.contains('~'), "{runbook}");
        assert!(!source.contains("rm -rf"), "{runbook}");
    }
    let fuzz_owners = fs::read_to_string(root.join("docs/references/p1-fuzz-ownership.md"))
        .expect("P1 fuzz ownership document");
    for owner in [
        "canonical bundle",
        "binding descriptor",
        "request metadata/header bridge",
        "resource/deployment/cursor ID codec",
        "facade RPC frame/structured value",
        "KV cursor and metadata",
        "D1 SQL authorizer and result encoder",
        "R2/S3 object key builder",
        "snapshot manifest/path parser",
        "migration/release metadata parser",
        "scheduler/DO internal envelope",
    ] {
        assert!(fuzz_owners.contains(owner), "missing fuzz owner {owner}");
    }

    let launcher = fs::read_to_string(root.join("scripts/package-release.sh")).unwrap();
    assert!(launcher.contains("exec bun"));
    let package = fs::read_to_string(root.join("scripts/package-release.ts")).unwrap();
    assert!(package.contains("[\"rev-parse\", \"--verify\", \"HEAD\"]"));
    assert!(package.contains("[\"status\", \"--porcelain\", \"--untracked-files=all\"]"));
    assert!(package.contains("OPEN_COMPUTE_GIT_REVISION: revision"));
    assert!(package.contains("OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE: pin.archive"));
    assert!(package.contains("await link(temporary, destination)"));
}
