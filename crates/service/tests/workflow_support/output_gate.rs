//! Native DO output-gate probe for the production Workflow facade.

use std::path::PathBuf;
use std::time::Duration;

#[tokio::test]
async fn workflow_do_mutation_fails_closed_after_native_output_gate_probe() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let binary = PathBuf::from(
        std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
            .expect("Workflow Gate requires verified stock workerd"),
    );
    open_compute_runtime::verify_runtime_binary(
        &root.join("runtime/workerd.lock.json"),
        &binary,
        Duration::from_secs(10),
        &open_compute_core::Redactor::new(),
    )
    .await
    .unwrap();
    let storage = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(binary)
            .arg("test")
            .arg("--experimental")
            .arg(format!(
                "--directory-path=storage={}",
                storage.path().display()
            ))
            .arg(root.join("crates/service/tests/workflow_support/output-gate.capnp"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
