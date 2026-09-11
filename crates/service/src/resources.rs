//! Read-only operator resources embedded in the single executable.

use open_compute_core::{ErrorCode, ObjectStorageConfig, PlatformConfig, PlatformError};
use std::io::Write;
use std::path::Path;

const DEFAULT_CONFIG: &str = include_str!("../../../share/default-config.toml");
const LICENSE: &str = include_str!("../../../LICENSE");
const WORKERD_LICENSE: &str = include_str!("../../../share/workerd-LICENSE");
const XBERG_LICENSE: &str = include_str!("../../../share/xberg-LICENSE");
const TESSERACT_LICENSE: &str = include_str!("../../../share/tesseract-LICENSE");
const LEPTONICA_LICENSE: &str = include_str!("../../../share/leptonica-LICENSE");

macro_rules! runbooks {
    ($($name:literal),+ $(,)?) => {
        const RUNBOOKS: &[(&str, &str)] = &[
            $(($name, include_str!(concat!("../../../docs/references/runbooks/", $name, ".md")))),+
        ];
    };
}

runbooks!(
    "backup-and-retention",
    "collect-support-bundle",
    "disk-pressure",
    "fresh-host-restore",
    "install-and-first-start",
    "master-key-loss-and-recovery",
    "s3-outage",
    "scheduler-recovery",
    "sqlite-corruption",
    "current-release-recovery",
    "workerd-crash-loop",
);

pub(crate) fn write_config(data_dir: &Path, out: &mut impl Write) -> Result<(), PlatformError> {
    let mut config = PlatformConfig::from_toml_str(DEFAULT_CONFIG)?;
    config.data.path = data_dir.to_owned();
    config.data.master_key_file = data_dir.join("keys/master.key");
    let ObjectStorageConfig::Local(local) = &mut config.object_storage else {
        return Err(invalid());
    };
    local.path = data_dir.join("objects");
    config.validate()?;
    let text = toml::to_string_pretty(&config).map_err(|_| invalid())?;
    writeln!(out, "# Local single-machine configuration. See the S3 reference before selecting a remote object authority.\n{text}")
        .map_err(|_| invalid())
}

pub(crate) fn write_licenses(out: &mut impl Write) -> Result<(), PlatformError> {
    writeln!(
        out,
        "Open Compute\n{LICENSE}\nEmbedded Cloudflare workerd\n{WORKERD_LICENSE}\nEmbedded Tesseract OCR\n{TESSERACT_LICENSE}\nEmbedded Leptonica\n{LEPTONICA_LICENSE}\nEmbedded tessdata_fast language models (eng, chi_sim, chi_tra; Apache-2.0; revision 87416418657359cb625c412a48b6e1d6d41c29bd)\n{TESSERACT_LICENSE}\nEmbedded Xberg document parser\n{XBERG_LICENSE}"
    )
    .map_err(|_| invalid())
}

pub(crate) fn write_docs(name: Option<&str>, out: &mut impl Write) -> Result<(), PlatformError> {
    match name {
        Some(name) => {
            let (_, content) = RUNBOOKS
                .iter()
                .find(|(key, _)| *key == name)
                .ok_or_else(invalid)?;
            write!(out, "{content}").map_err(|_| invalid())
        }
        None => {
            for (name, _) in RUNBOOKS {
                writeln!(out, "{name}").map_err(|_| invalid())?;
            }
            Ok(())
        }
    }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "embedded operator resource is invalid or output failed",
    )
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
