//! Embed the release-owned operator dashboard static assets.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 20_000;

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let dist = root.join("packages/dashboard/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    if !fs::symlink_metadata(&dist)?.is_dir() {
        return Err("packages/dashboard/dist is missing; run bun run build before Cargo".into());
    }
    let mut files = BTreeMap::new();
    collect(&dist, &dist, &mut files)?;
    if files.is_empty() {
        return Err("packages/dashboard/dist is empty; run bun run build before Cargo".into());
    }
    if files.len() > MAX_FILES {
        return Err("embedded dashboard exceeds its file-count bound".into());
    }
    let total: u64 = files.values().map(|bytes| bytes.len() as u64).sum();
    if total > MAX_TOTAL_BYTES {
        return Err("embedded dashboard exceeds its total-size bound".into());
    }

    let mut digest = Sha256::new();
    digest.update(b"open-compute/embedded-dashboard/v1\0");
    for (path, bytes) in &files {
        put(&mut digest, path.as_bytes());
        put(&mut digest, bytes);
    }
    let assets_hash = hex::encode(digest.finalize());

    let out = PathBuf::from(env::var("OUT_DIR")?);
    let mut source = format!(
        "pub(super) const ASSETS_SHA256: &str = {assets_hash:?};\n\
         pub(super) const FILES: &[(&str, &[u8])] = &[\n"
    );
    for (index, (path, bytes)) in files.iter().enumerate() {
        fs::write(out.join(format!("dashboard-{index}")), bytes)?;
        source.push_str(&format!(
            "({path:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/dashboard-{index}\"))),\n"
        ));
    }
    source.push_str("];\n");
    fs::write(out.join("embedded_dashboard.rs"), source)?;
    Ok(())
}

fn put(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn collect(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "dashboard asset path is not UTF-8")?;
        let path = current.join(&name);
        if entry.file_type()?.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(
                format!("dashboard asset is not a regular file: {}", path.display()).into(),
            );
        }
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .ok_or("dashboard asset path is not UTF-8")?
            .replace('\\', "/");
        let bytes = read_bounded(&path, MAX_FILE_BYTES)?;
        files.insert(relative, bytes);
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!("dashboard asset exceeds its size bound: {}", path.display()).into());
    }
    Ok(fs::read(path)?)
}
