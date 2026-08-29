//! Cleanup for exact SQLite files in the platform-owned backup staging directory.

use std::path::{Path, PathBuf};

pub(crate) fn remove_sqlite_staging(path: &Path) {
    for suffix in ["-shm", "-wal", "-journal", ""] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}
