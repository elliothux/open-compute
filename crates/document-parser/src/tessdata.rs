use crate::{DocumentErrorCode, DocumentParserError, error, sha256_hex};
use flate2::read::GzDecoder;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

/// SHA-256 of the canonical tessdata lock used by this build.
pub const TESSDATA_CONTRACT_SHA256: &str = env!("OPEN_COMPUTE_TESSDATA_LOCK_SHA256");
/// Fixed OCR language expression. Arbitrary language selection is not supported.
pub const OCR_LANGUAGE_EXPRESSION: &str = "eng+chi_sim+chi_tra";

struct Asset {
    name: &'static str,
    size: usize,
    sha256: &'static str,
    compressed: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset {
        name: "eng.traineddata",
        size: 4_113_088,
        sha256: "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
        compressed: include_bytes!(concat!(env!("OUT_DIR"), "/eng.traineddata.gz")),
    },
    Asset {
        name: "chi_sim.traineddata",
        size: 2_469_156,
        sha256: "a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730",
        compressed: include_bytes!(concat!(env!("OUT_DIR"), "/chi_sim.traineddata.gz")),
    },
    Asset {
        name: "chi_tra.traineddata",
        size: 2_366_642,
        sha256: "529c5b5797d64b126065cd55f2bb4c7fd7b15790798091b1ff259941a829330b",
        compressed: include_bytes!(concat!(env!("OUT_DIR"), "/chi_tra.traineddata.gz")),
    },
];

const ALIASES: &[(&str, &str)] = &[
    ("zho.traineddata", "chi_sim.traineddata"),
    ("chinese_cht.traineddata", "chi_tra.traineddata"),
];

/// Materialize the embedded, digest-verified OCR assets below an exclusively owned data root.
pub fn materialize_tessdata(data_root: &Path) -> Result<PathBuf, DocumentParserError> {
    if !data_root.is_absolute() || !is_regular_directory(data_root)? {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    let parent = data_root.join("tessdata");
    ensure_private_directory(&parent)?;
    let destination = parent.join(TESSDATA_CONTRACT_SHA256);
    ensure_private_directory(&destination)?;
    for asset in ASSETS {
        materialize_asset(&destination, asset)?;
    }
    for (alias, source) in ALIASES {
        materialize_alias(&destination, alias, source)?;
    }
    verify_tessdata_dir(&destination)?;
    sync_directory(&destination)?;
    sync_directory(&parent)?;
    Ok(destination)
}

/// Verify the exact three-language asset set at a parent-resolved directory.
pub fn verify_tessdata_dir(path: &Path) -> Result<(), DocumentParserError> {
    if !path.is_absolute() || !is_regular_directory(path)? {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    let mut names = fs::read_dir(path)
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?
        .map(|entry| {
            entry
                .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))
                .and_then(|entry| {
                    let metadata = entry
                        .metadata()
                        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
                    if !metadata.is_file()
                        || entry.file_type().map_or(true, |kind| kind.is_symlink())
                    {
                        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
                    }
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected = ASSETS
        .iter()
        .map(|asset| asset.name.to_owned())
        .collect::<Vec<_>>();
    expected.extend(ALIASES.iter().map(|(alias, _)| (*alias).to_owned()));
    expected.sort();
    if names != expected {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    for asset in ASSETS {
        verify_asset(&path.join(asset.name), asset)?;
    }
    for (alias, source) in ALIASES {
        let asset = ASSETS
            .iter()
            .find(|asset| asset.name == *source)
            .ok_or_else(|| error(DocumentErrorCode::DocumentOcrUnavailable))?;
        verify_asset(&path.join(alias), asset)?;
    }
    Ok(())
}

fn materialize_alias(
    destination: &Path,
    alias: &str,
    source: &str,
) -> Result<(), DocumentParserError> {
    let asset = ASSETS
        .iter()
        .find(|asset| asset.name == source)
        .ok_or_else(|| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    let alias_path = destination.join(alias);
    match fs::symlink_metadata(&alias_path) {
        Ok(_) => return verify_asset(&alias_path, asset),
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error(DocumentErrorCode::DocumentOcrUnavailable)),
    }
    fs::hard_link(destination.join(source), &alias_path)
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    verify_asset(&alias_path, asset)
}

fn materialize_asset(destination: &Path, asset: &Asset) -> Result<(), DocumentParserError> {
    let final_path = destination.join(asset.name);
    match fs::symlink_metadata(&final_path) {
        Ok(_) => return verify_asset(&final_path, asset),
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error(DocumentErrorCode::DocumentOcrUnavailable)),
    }
    let bytes = decompress(asset)?;
    let temporary = destination.join(format!(".{}.{}.tmp", asset.name, std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
        file.sync_all()
            .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
        drop(file);
        fs::rename(&temporary, &final_path)
            .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
        sync_directory(destination)?;
        verify_asset(&final_path, asset)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn decompress(asset: &Asset) -> Result<Vec<u8>, DocumentParserError> {
    let mut bytes = Vec::with_capacity(asset.size);
    GzDecoder::new(asset.compressed)
        .take(u64::try_from(asset.size + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    if bytes.len() != asset.size || sha256_hex(&bytes) != asset.sha256 {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    Ok(bytes)
}

fn verify_asset(path: &Path, asset: &Asset) -> Result<(), DocumentParserError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    let mut bytes = Vec::with_capacity(asset.size);
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    if bytes.len() != asset.size || sha256_hex(&bytes) != asset.sha256 {
        return Err(error(DocumentErrorCode::DocumentOcrUnavailable));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), DocumentParserError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if metadata.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
            }
        }
        Ok(_) => return Err(error(DocumentErrorCode::DocumentOcrUnavailable)),
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
        }
        Err(_) => return Err(error(DocumentErrorCode::DocumentOcrUnavailable)),
    }
    Ok(())
}

fn is_regular_directory(path: &Path) -> Result<bool, DocumentParserError> {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))
}

fn sync_directory(path: &Path) -> Result<(), DocumentParserError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))
}
