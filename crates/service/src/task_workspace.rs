//! Private bounded-child workspaces with identity-checked crash recovery.

use open_compute_core::{ErrorCode, PlatformError};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const OWNER: &str = "ocd-task-v1";
const COMPLETE: &[u8] = b"ocd-task-complete-v1\n";

pub(crate) fn create(root: &Path, prefix: &str) -> Result<tempfile::TempDir, PlatformError> {
    let workspace = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(root)
        .map_err(|_| invalid())?;
    let directory = rustix::fs::open(
        workspace.path(),
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| invalid())?;
    rustix::fs::fchmod(&directory, rustix::fs::Mode::RWXU).map_err(|_| invalid())?;
    open_compute_storage::ensure_dir_secure(workspace.path())?;
    let owner = format!("{OWNER}\n{}\n", std::process::id());
    open_compute_storage::atomic_write(&workspace.path().join(".owner"), owner.as_bytes())?;
    Ok(workspace)
}

pub(crate) fn mark_completed(workspace: &Path) -> Result<(), PlatformError> {
    open_compute_storage::atomic_write(&workspace.join(".completed"), COMPLETE)
}

pub(crate) fn recover(
    root: &Path,
    prefixes: &[&str],
    lease_name: &str,
    digest: &str,
) -> Result<(), PlatformError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(invalid()),
        Ok(_) => open_compute_storage::ensure_dir_secure(root)?,
    }
    for entry in fs::read_dir(root).map_err(|_| invalid())? {
        let path = entry.map_err(|_| invalid())?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            continue;
        }
        let Some(pid) = owner_pid(&path) else {
            continue;
        };
        let Some(pid) = rustix::process::Pid::from_raw(pid) else {
            continue;
        };
        match rustix::process::test_kill_process(pid) {
            Ok(()) => continue,
            Err(rustix::io::Errno::SRCH) => {}
            Err(_) => continue,
        }
        let lease = path.join(lease_name);
        if present_or_unknown(&lease) || present_or_unknown(&lease.with_extension("staging")) {
            open_compute_runtime::PersistentHostProcess::recover_orphan(&lease, digest)?;
        } else if !completed(&path) {
            continue;
        }
        fs::remove_dir_all(&path).map_err(|_| invalid())?;
    }
    Ok(())
}

fn owner_pid(path: &Path) -> Option<i32> {
    let bytes = read_marker(&path.join(".owner"), 128)?;
    let body = std::str::from_utf8(&bytes).ok()?;
    let pid = body
        .strip_prefix(&format!("{OWNER}\n"))?
        .strip_suffix('\n')?;
    pid.parse::<i32>().ok().filter(|pid| *pid > 1)
}

fn completed(path: &Path) -> bool {
    read_marker(&path.join(".completed"), 64).is_some_and(|bytes| bytes == COMPLETE)
}

fn read_marker(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    open_compute_storage::validate_owned_file(path, true).ok()?;
    if fs::symlink_metadata(path).ok()?.len() > max_bytes {
        return None;
    }
    fs::read(path).ok()
}

fn present_or_unknown(path: &Path) -> bool {
    !matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::PathInvalid,
        "private task workspace is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::Redactor;
    use open_compute_runtime::{
        HostProcessLease, HostProcessSpec, VerifiedLaunchImage, run_host_process,
    };
    use sha2::{Digest as _, Sha256};
    use std::ffi::OsString;
    use std::fs::File;
    use std::time::{Duration, Instant};

    #[test]
    fn recovery_preserves_active_and_unknown_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("tmp");
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let stale = root.join("caddy-tool-stale");
        let incomplete = root.join("caddy-tool-incomplete");
        let active = root.join("caddy-tool-active");
        let unknown = root.join("caddy-tool-unknown");
        for path in [&stale, &incomplete, &active, &unknown] {
            open_compute_storage::ensure_dir_secure(path).unwrap();
        }
        open_compute_storage::atomic_write(
            &stale.join(".owner"),
            format!("{OWNER}\n{}\n", i32::MAX).as_bytes(),
        )
        .unwrap();
        open_compute_storage::atomic_write(
            &incomplete.join(".owner"),
            format!("{OWNER}\n{}\n", i32::MAX).as_bytes(),
        )
        .unwrap();
        open_compute_storage::atomic_write(
            &active.join(".owner"),
            format!("{OWNER}\n{}\n", std::process::id()).as_bytes(),
        )
        .unwrap();
        mark_completed(&stale).unwrap();
        mark_completed(&active).unwrap();
        mark_completed(&unknown).unwrap();
        recover(&root, &["caddy-tool-"], "tool.lease", &"a".repeat(64)).unwrap();
        assert!(!stale.exists());
        assert!(incomplete.exists());
        assert!(active.exists());
        assert!(unknown.exists());
    }

    #[test]
    fn recovery_clears_a_dead_verified_child_lease_before_workspace_deletion() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("tmp");
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let workspace = root.join("document-parser-stale");
        open_compute_storage::ensure_dir_secure(&workspace).unwrap();
        open_compute_storage::atomic_write(
            &workspace.join(".owner"),
            format!("{OWNER}\n{}\n", i32::MAX).as_bytes(),
        )
        .unwrap();
        let digest = "a".repeat(64);
        let lease = serde_json::json!({
            "schema_version": 1,
            "pid": i32::MAX,
            "pgid": i32::MAX,
            "start_key": "former-child",
            "binary_sha256": digest,
        });
        open_compute_storage::atomic_write(
            &workspace.join("child.lease"),
            &serde_json::to_vec(&lease).unwrap(),
        )
        .unwrap();
        recover(&root, &["document-parser-"], "child.lease", &digest).unwrap();
        assert!(!workspace.exists());
    }

    #[tokio::test]
    async fn task_workspace_crash_helper() {
        let Some(root) = std::env::var_os("OC_TEST_TASK_WORKSPACE_ROOT") else {
            return;
        };
        let prefix = std::env::var("OC_TEST_TASK_WORKSPACE_PREFIX").unwrap();
        let lease_name = std::env::var("OC_TEST_TASK_WORKSPACE_LEASE").unwrap();
        let workspace = create(Path::new(&root), &prefix).unwrap();
        let executable = Path::new("/bin/sleep");
        let digest = hex::encode(Sha256::digest(fs::read(executable).unwrap()));
        run_host_process(
            &VerifiedLaunchImage::from_verified_file(File::open(executable).unwrap()),
            HostProcessSpec {
                args: vec![OsString::from("30")],
                environment: Vec::new(),
                working_directory: workspace.path().to_owned(),
                stdin: Vec::new(),
                deadline: Duration::from_secs(35),
                max_stdout: 0,
                max_stderr: 0,
                redactor: Redactor::new(),
                lease: Some(HostProcessLease {
                    path: workspace.path().join(lease_name),
                    binary_sha256: digest,
                }),
            },
        )
        .await
        .unwrap();
    }

    #[test]
    fn caddy_and_parser_workspaces_recover_after_real_owner_sigkill() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("tmp");
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let digest = hex::encode(Sha256::digest(fs::read("/bin/sleep").unwrap()));
        for (prefix, lease_name) in [
            ("caddy-tool-", "tool.lease"),
            ("document-parser-", "child.lease"),
        ] {
            let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "task_workspace::tests::task_workspace_crash_helper",
                ])
                .env("OC_TEST_TASK_WORKSPACE_ROOT", &root)
                .env("OC_TEST_TASK_WORKSPACE_PREFIX", prefix)
                .env("OC_TEST_TASK_WORKSPACE_LEASE", lease_name)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let workspace = loop {
                let candidate = fs::read_dir(&root)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(prefix))
                            && path.join(lease_name).exists()
                    });
                if let Some(workspace) = candidate {
                    break workspace;
                }
                if owner.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                    let _ = owner.kill();
                    let _ = owner.wait();
                    panic!("task owner exited before writing its child lease");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let lease: serde_json::Value =
                serde_json::from_slice(&fs::read(workspace.join(lease_name)).unwrap()).unwrap();
            let child_pid = rustix::process::Pid::from_raw(
                i32::try_from(lease["pid"].as_i64().unwrap()).unwrap(),
            )
            .unwrap();
            owner.kill().unwrap();
            owner.wait().unwrap();
            assert_eq!(owner_pid(&workspace), Some(owner.id() as i32));
            assert!(workspace.join(lease_name).exists());
            let metadata = fs::symlink_metadata(&workspace).unwrap();
            assert_eq!(metadata.uid(), rustix::process::getuid().as_raw());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
            assert!(matches!(
                rustix::process::test_kill_process(
                    rustix::process::Pid::from_raw(owner.id() as i32).unwrap()
                ),
                Err(rustix::io::Errno::SRCH)
            ));
            recover(&root, &[prefix], lease_name, &digest).unwrap();
            assert!(
                !workspace.exists(),
                "workspace was not reclaimed: {workspace:?}"
            );
            assert!(matches!(
                rustix::process::test_kill_process(child_pid),
                Err(rustix::io::Errno::SRCH)
            ));
        }
    }
}
