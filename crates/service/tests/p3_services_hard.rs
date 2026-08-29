//! Stock-workerd hard gate for the native Service RPC trampoline and capability lifecycle.

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn p3_services_native_rpc_type_pipeline_and_lifecycle_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    assert!(
        workerd.is_absolute(),
        "OPEN_COMPUTE_TEST_WORKERD must be absolute"
    );
    assert!(
        workerd.is_file(),
        "OPEN_COMPUTE_TEST_WORKERD must be a file"
    );
    let fixture = repo_root().join("test/runtime/fixtures/service-bindings");
    let output = Command::new(workerd)
        .args(["test", "--experimental", "config.capnp"])
        .current_dir(&fixture)
        .output()
        .expect("run stock workerd Service RPC hard gate");
    assert!(
        output.status.success(),
        "stock workerd Service RPC hard gate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
}
