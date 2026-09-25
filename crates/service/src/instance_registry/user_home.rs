//! UID-owned home resolution, independent of ambient HOME.

use super::*;
use std::process::Command;

pub(crate) fn user_home_for_uid() -> Result<PathBuf, PlatformError> {
    let uid = rustix::process::getuid().as_raw();
    #[cfg(target_os = "macos")]
    let output = Command::new("/usr/bin/id").arg("-P").output();
    #[cfg(target_os = "linux")]
    let output = Command::new("/usr/bin/getent")
        .args(["passwd", &uid.to_string()])
        .output();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let output: std::io::Result<std::process::Output> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported host",
    ));
    let output = output.map_err(|_| manifest_invalid("failed to resolve UID home directory"))?;
    if !output.status.success() {
        return Err(manifest_invalid("failed to resolve UID home directory"));
    }
    let passwd = std::str::from_utf8(&output.stdout)
        .map_err(|_| manifest_invalid("UID passwd record is not UTF-8"))?;
    passwd_home(passwd, uid)
}

pub(super) fn passwd_home(record: &str, uid: u32) -> Result<PathBuf, PlatformError> {
    let fields: Vec<_> = record.trim_end_matches('\n').split(':').collect();
    if fields.len() != 10 && fields.len() != 7 {
        return Err(manifest_invalid("UID passwd record has invalid fields"));
    }
    if fields[2].parse::<u32>().ok() != Some(uid) {
        return Err(manifest_invalid(
            "UID passwd record does not match running UID",
        ));
    }
    let home = PathBuf::from(fields[fields.len() - 2]);
    if !home.is_absolute()
        || home
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(manifest_invalid("UID home directory is invalid"));
    }
    Ok(home)
}
