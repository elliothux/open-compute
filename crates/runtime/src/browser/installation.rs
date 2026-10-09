//! Browser installation identity; startup never searches PATH or downloads assets.

use crate::{HostProcessSpec, VerifiedLaunchImage, run_host_process};
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Opened browser executable and the qualified sibling resources for this host.
#[derive(Debug)]
pub struct BrowserInstallation {
    pub(crate) image: VerifiedLaunchImage,
    pub(super) frontend: Arc<super::BrowserFrontend>,
    /// Exact opened executable digest used by process recovery.
    pub binary_sha256: String,
    /// Nonsecret runtime identity including resources, version, and transport policy.
    pub contract_sha256: String,
    /// Qualified version output, without paths or process details.
    pub version: String,
}

impl BrowserInstallation {
    /// Borrow verified executable/resources for an operator-owned native CDP test fixture.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn launch_image_for_test(&self) -> &VerifiedLaunchImage {
        &self.image
    }

    /// Verify an explicit complete Chrome Headless Shell installation.
    /// `workspace` is a pre-created private generation directory owned by the caller.
    pub async fn open(executable: &Path, workspace: &Path) -> Result<Self, PlatformError> {
        let root = executable.parent().ok_or_else(invalid)?;
        let _ = crate::fsutil::open_dir_nofollow(root)?;
        let _ = crate::fsutil::open_dir_nofollow(workspace)?;
        let mut file = crate::fsutil::open_nofollow(executable, false, false)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o100 == 0
            || metadata.permissions().mode() & 0o022 != 0
            || !(1..=1024 * 1024 * 1024).contains(&metadata.len())
        {
            return Err(invalid());
        }
        let binary_sha256 = digest(&mut file)?;
        let mut contract = Sha256::new();
        contract.update(
            b"open-compute/browser-cdp-pipe/context-scope/site-per-process/scoped-downloads/scoped-windows\0",
        );
        contract.update(binary_sha256.as_bytes());
        let mut resources = Vec::new();
        for name in [
            "headless_lib_strings.pak",
            "headless_lib_data.pak",
            "headless_command_resources.pak",
            "icudtl.dat",
            snapshot_name(),
        ] {
            let mut resource = crate::fsutil::open_nofollow(&root.join(name), false, false)?;
            let metadata = resource.metadata().map_err(|_| invalid())?;
            if !metadata.is_file()
                || metadata.permissions().mode() & 0o022 != 0
                || !(1..=64 * 1024 * 1024).contains(&metadata.len())
            {
                return Err(invalid());
            }
            contract.update(name.as_bytes());
            contract.update(digest(&mut resource)?.as_bytes());
            resources.push((name.to_owned(), resource));
        }
        let image = VerifiedLaunchImage::from_verified_resources(file, resources)?;
        let result = run_host_process(
            &image,
            HostProcessSpec {
                args: vec!["--version".into()],
                environment: Vec::new(),
                working_directory: workspace.to_owned(),
                stdin: Vec::new(),
                deadline: Duration::from_secs(5),
                max_stdout: 1024,
                max_stderr: 4096,
                redactor: Redactor::new(),
                lease: None,
            },
        )
        .await?;
        if result.timed_out
            || result.stdout_overflow
            || result.stderr_overflow
            || !result.status.is_some_and(|status| status.success())
        {
            return Err(invalid());
        }
        let version = std::str::from_utf8(&result.stdout)
            .map_err(|_| invalid())?
            .trim();
        if !version
            .strip_prefix("Google Chrome for Testing ")
            .is_some_and(|number| {
                let pieces: Vec<_> = number.split('.').collect();
                pieces.len() == 4
                    && pieces[0] == "153"
                    && pieces.iter().all(|piece| {
                        !piece.is_empty() && piece.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
        {
            return Err(invalid());
        }
        contract.update(version.as_bytes());
        let frontend = Arc::new(super::BrowserFrontend::open(
            &root.join("browser-devtools.json.gz"),
            &binary_sha256,
            version,
        )?);
        contract.update(frontend.sha256.as_bytes());
        Ok(Self {
            image,
            frontend,
            binary_sha256,
            contract_sha256: hex::encode(contract.finalize()),
            version: version.to_owned(),
        })
    }
}

fn snapshot_name() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "v8_context_snapshot.arm64.bin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "v8_context_snapshot.x86_64.bin"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "v8_context_snapshot.bin"
    }
}

fn digest(file: &mut File) -> Result<String, PlatformError> {
    file.rewind().map_err(|_| invalid())?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut bytes).map_err(|_| invalid())?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    file.rewind().map_err(|_| invalid())?;
    Ok(hex::encode(hash.finalize()))
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "browser installation is not qualified",
    )
}
