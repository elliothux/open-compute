use super::{ADMIN_TOKEN, READ_ONLY_TOKEN, S3_ACCESS_KEY, S3_SECRET_KEY, TAIL_SECRET, TOKEN};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

pub(super) struct Evidence {
    temp: Option<tempfile::TempDir>,
}

impl Evidence {
    pub(super) fn new(temp: tempfile::TempDir) -> Self {
        Self { temp: Some(temp) }
    }

    pub(super) fn path(&self) -> &Path {
        self.temp.as_ref().unwrap().path()
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let Some(temp) = self.temp.take() else {
            return;
        };
        let path = temp.keep();
        sanitize_failure_evidence(&path);
        let failed = path.parent().unwrap().join("failed");
        let _ = fs::create_dir_all(&failed);
        let _ = fs::set_permissions(&failed, fs::Permissions::from_mode(0o700));
        let _ = fs::rename(&path, failed.join(path.file_name().unwrap()));
    }
}

fn sanitize_failure_evidence(path: &Path) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            sanitize_failure_evidence(&child);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let _ = fs::set_permissions(&child, fs::Permissions::from_mode(0o600));
        let sensitive_file = matches!(
            child.file_name().and_then(|name| name.to_str()),
            Some(
                "access-key"
                    | "secret-key"
                    | "admin.token"
                    | "deployer.token"
                    | "read-only.token"
                    | "master.key"
            )
        );
        if sensitive_file {
            let _ = fs::write(&child, b"[REDACTED]\n");
            continue;
        }
        let Ok(mut bytes) = fs::read(&child) else {
            continue;
        };
        let mut changed = false;
        for secret in known_secrets() {
            changed |= redact_bytes(&mut bytes, secret.as_bytes());
        }
        if changed {
            let _ = fs::write(&child, bytes);
        }
    }
}

fn redact_bytes(haystack: &mut [u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let mut changed = false;
    let mut offset = 0;
    while let Some(relative) = haystack[offset..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = offset + relative;
        haystack[start..start + needle.len()].fill(b'*');
        offset = start + needle.len();
        changed = true;
    }
    changed
}

pub(super) fn known_secrets() -> [&'static str; 6] {
    [
        ADMIN_TOKEN,
        TOKEN,
        READ_ONLY_TOKEN,
        S3_ACCESS_KEY,
        S3_SECRET_KEY,
        TAIL_SECRET,
    ]
}
