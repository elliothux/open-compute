//! SIGKILL coverage for current durable decisions and both-database lifecycle ownership.

use super::*;
use std::io::{BufRead as _, Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const MARKER: &str = "WORKFLOW_CRASH_BOUNDARY";

#[path = "workflow_durable_crash_tests.rs"]
mod durable;

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1024 * 1024 * 1024,
        free_space_hard_bytes: 256 * 1024 * 1024,
    }
}

fn checkpoint(selected: &str, point: &str) {
    if selected == point {
        println!("{MARKER}");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        let _ = std::io::stdin().read_exact(&mut byte);
        panic!("crash parent closed the pipe without killing the fixture");
    }
}

struct Evidence(Option<tempfile::TempDir>);

impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(temp) = self.0.take()
        {
            let path = temp.keep();
            let failed =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/workflow-run/failed");
            std::fs::create_dir_all(&failed).unwrap();
            let destination = failed.join(format!("workflow-saga-{}", RequestId::generate()));
            std::fs::rename(&path, &destination).unwrap();
            eprintln!("Workflow saga failure evidence: {}", destination.display());
        }
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn kill_at_boundary(root: &Path, definition: WorkflowId, cut: &str, child_test: &str) {
    let stderr = std::fs::File::create(root.join("crash-child.stderr")).unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", child_test, "--nocapture", "--test-threads=1"])
            .env("OPEN_COMPUTE_WORKFLOW_CRASH_DATA", root)
            .env("OPEN_COMPUTE_WORKFLOW_CRASH_POINT", cut)
            .env(
                "OPEN_COMPUTE_WORKFLOW_CRASH_DEFINITION",
                definition.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let reached = std::io::BufReader::new(stdout)
            .lines()
            .any(|line| line.is_ok_and(|line| line.contains(MARKER)));
        let _ = sender.send(reached);
    });
    let reached = receiver.recv_timeout(Duration::from_secs(30));
    if child.0.try_wait().unwrap().is_none() {
        child.0.kill().unwrap();
    }
    let status = child.0.wait().unwrap();
    reader.join().unwrap();
    assert!(
        matches!(reached, Ok(true)),
        "checkpoint {cut}: {reached:?}; {status}"
    );
    assert_eq!(status.signal(), Some(9), "{cut}");
}
