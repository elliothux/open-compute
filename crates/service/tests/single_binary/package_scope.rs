use open_compute_service::instance_registry::{InstanceRegistry, ServiceScope};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn enabled() -> bool {
    std::env::var("OPEN_COMPUTE_PACKAGE_GATE_USER_ROOT").as_deref() == Ok("1")
}

pub(super) fn registry(root: &Path) -> InstanceRegistry {
    if enabled() {
        InstanceRegistry::production().unwrap()
    } else {
        InstanceRegistry::with_roots(root.join("test-ocd/system"), root.join("test-ocd/user"))
    }
}

pub(super) struct UserRoot {
    path: Option<PathBuf>,
    evidence: PathBuf,
}

impl UserRoot {
    pub(super) fn reserve(evidence: &Path) -> Self {
        if !enabled() {
            return Self {
                path: None,
                evidence: evidence.to_path_buf(),
            };
        }
        assert!(
            std::env::var_os("OPEN_COMPUTE_TEST_OCD").is_some(),
            "the package user-root gate requires an external release binary"
        );
        assert!(
            std::env::var_os("OPEN_COMPUTE_TEST_OCD_ROOT").is_none(),
            "the package user-root gate must exercise production root selection"
        );
        let path = registry(evidence)
            .root_for(ServiceScope::User)
            .to_path_buf();
        assert!(
            !path.exists(),
            "the package user-root gate refuses to modify an existing OCD_DIR: {}",
            path.display()
        );
        Self {
            path: Some(path),
            evidence: evidence.to_path_buf(),
        }
    }
}

impl Drop for UserRoot {
    fn drop(&mut self) {
        let Some(path) = &self.path else { return };
        if !path.exists() {
            return;
        }
        if std::thread::panicking() && fs::rename(path, self.evidence.join("user-ocd")).is_ok() {
            return;
        }
        fs::remove_dir_all(path).unwrap();
    }
}
