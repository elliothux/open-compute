//! Acceptance: no resolved package declares rust-version newer than workspace MSRV.

use serde_json::Value;
use std::process::Command;

#[test]
fn no_resolved_package_exceeds_workspace_msrv() {
    let workspace_msrv = env!("CARGO_PKG_RUST_VERSION");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: Value = serde_json::from_slice(&output.stdout).expect("json");
    let mut bad = Vec::new();
    for pkg in meta["packages"].as_array().unwrap() {
        let Some(rv) = pkg["rust_version"].as_str() else {
            continue;
        };
        if version_tuple(rv) > version_tuple(workspace_msrv) {
            bad.push(format!(
                "{} {} rust-version {rv}",
                pkg["name"].as_str().unwrap_or("?"),
                pkg["version"].as_str().unwrap_or("?")
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "packages newer than rust {workspace_msrv}:\n{}",
        bad.join("\n")
    );
}

fn version_tuple(s: &str) -> (u64, u64, u64) {
    let mut parts = s.split('.').map(|p| p.parse().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
