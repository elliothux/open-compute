use super::RESOURCE_PROBE_COMMAND;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn resource_marker_is_visible_only_after_its_complete_write() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("run")).unwrap();
    let publish = RESOURCE_PROBE_COMMAND
        .split_once("umask 027; ")
        .unwrap()
        .1
        .split_once("; while :; do :; done")
        .unwrap()
        .0
        .replace(
            "/run/open-compute-snapshot-probe",
            "\"$MARKER_TEST_ROOT/run/marker\"",
        )
        .replace(
            "/open-compute-snapshot-probe",
            "\"$MARKER_TEST_ROOT/marker\"",
        );
    // Shell redirection happens before printf enters the controlled write barrier.
    let script = concat!(
        "set -eu; count=0; printf() { count=$((count+1)); ",
        "if [ \"$count\" -eq 2 ]; then : > \"$MARKER_TEST_ROOT/entered\"; ",
        "while [ ! -e \"$MARKER_TEST_ROOT/release\" ]; do sleep 0.01; done; fi; ",
        "command printf \"$@\"; }; umask 027; "
    )
    .to_owned()
        + &publish;
    let mut child = ChildGuard(
        Command::new("sh")
            .args(["-c", &script])
            .env("MARKER_TEST_ROOT", root)
            .env(
                "OPEN_COMPUTE_QUALIFICATION_NONCE",
                "controlled-public-nonce",
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let entered = root.join("entered");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !entered.exists() && Instant::now() < deadline {
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let marker = root.join("marker");
    let visible_before_write = marker.exists();
    std::fs::write(root.join("release"), []).unwrap();
    let status = child.0.wait().unwrap();
    assert!(entered.exists() && status.success());
    assert!(
        !visible_before_write,
        "readiness must not expose an empty marker"
    );
    assert_eq!(std::fs::read(&marker).unwrap(), b"controlled-public-nonce");
    assert_eq!(
        std::fs::metadata(marker).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(!root.join("marker.pending").exists());
}
