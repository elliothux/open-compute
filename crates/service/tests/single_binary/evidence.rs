use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

pub(super) struct Evidence(Option<TempDir>);

impl Evidence {
    pub(super) fn new() -> Self {
        Self(Some(
            tempfile::Builder::new()
                .prefix("single-")
                .tempdir_in("/tmp")
                .unwrap(),
        ))
    }

    pub(super) fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(temp) = self.0.take()
        {
            let path = temp.keep();
            let failed =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/single-binary-run/failed");
            if fs::create_dir_all(&failed).is_ok() {
                let destination = failed.join(path.file_name().unwrap());
                if retain_directory(&path, &destination).is_ok() {
                    eprintln!("single-binary failure evidence: {}", destination.display());
                    return;
                }
            }
            eprintln!("single-binary failure evidence: {}", path.display());
        }
    }
}

pub(super) fn retain_directory(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_directory(source, destination)?;
            fs::remove_dir_all(source)
        }
        Err(error) => Err(error),
    }
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    let status = Command::new("/bin/cp")
        .arg("-a")
        .arg(source.join("."))
        .arg(destination)
        .status()?;
    if !status.success() {
        return Err(io::Error::other("failure evidence copy failed"));
    }
    Ok(())
}

#[test]
fn failure_evidence_copy_preserves_files_modes_and_symlinks() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let file = source.path().join("marker");
    fs::write(&file, b"retained evidence").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    symlink("marker", source.path().join("alias")).unwrap();
    copy_directory(source.path(), destination.path()).unwrap();
    assert_eq!(
        fs::read(destination.path().join("marker")).unwrap(),
        b"retained evidence"
    );
    assert_eq!(
        fs::read_link(destination.path().join("alias")).unwrap(),
        Path::new("marker")
    );
    assert_eq!(
        fs::metadata(destination.path().join("marker"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(source.path().join("marker").exists());
    assert!(retain_directory(source.path(), destination.path()).is_err());
}
