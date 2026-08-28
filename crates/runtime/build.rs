//! Build the mandatory, target-specific offline runtime payload.

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let target = match env::var("TARGET")?.as_str() {
        "aarch64-apple-darwin" => "darwin-arm64",
        "x86_64-apple-darwin" => "darwin-x64",
        "aarch64-unknown-linux-gnu" => "linux-arm64",
        "x86_64-unknown-linux-gnu" => "linux-x64",
        _ => return Err("unsupported embedded workerd build target".into()),
    };
    let archive_path = PathBuf::from(env::var("OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE").map_err(
        |_| "set OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE to the absolute path of the formally pinned official .gz archive; builds never download or search for workerd",
    )?);
    if !archive_path.is_absolute() || !fs::metadata(&archive_path)?.is_file() {
        return Err("OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE must name an absolute regular file".into());
    }
    println!("cargo:rerun-if-changed={}", archive_path.display());
    let archive = read_bounded(&archive_path, MAX_ARCHIVE)?;
    let lock_bytes = tracked(&root.join("packages/runtime/workerd.lock.json"))?;
    let lock: serde_json::Value = serde_json::from_slice(&lock_bytes)?;
    let selected = &lock["targets"][target];
    let archive_hash = hex::encode(Sha256::digest(&archive));
    if selected["archiveSha256"].as_str() != Some(&archive_hash) {
        return Err("official archive SHA-256 does not match the build target's formal pin".into());
    }
    let mut decoder = GzDecoder::new(archive.as_slice()).take(MAX_BINARY + 1);
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let count = decoder.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&chunk[..count]);
    }
    if size > MAX_BINARY
        || selected["binarySha256"].as_str() != Some(&hex::encode(hasher.finalize()))
    {
        return Err(
            "decompressed workerd exceeds its bound or does not match the formal pin".into(),
        );
    }

    let mut assets = BTreeMap::new();
    assets.insert("runtime/workerd.lock.json".to_owned(), lock_bytes);
    assets.insert(
        "runtime/config.capnp".to_owned(),
        tracked(&root.join("packages/runtime/config.capnp"))?,
    );
    collect(&root.join("packages"), "runtime/dist", &mut assets)?;
    verify_manifest(&root, &assets)?;

    let mut digest = Sha256::new();
    digest.update(b"open-compute/embedded-runtime/v1\0");
    put(&mut digest, target.as_bytes());
    put(&mut digest, archive_hash.as_bytes());
    for (name, bytes) in &assets {
        put(&mut digest, name.as_bytes());
        put(&mut digest, bytes);
    }
    let payload_hash = hex::encode(digest.finalize());
    let mut assets_digest = Sha256::new();
    assets_digest.update(b"open-compute/runtime-assets/v1\0");
    put(&mut assets_digest, &assets["runtime/config.capnp"]);
    let workers: Vec<_> = assets
        .iter()
        .filter(|(path, _)| path.starts_with("runtime/dist/"))
        .collect();
    assets_digest.update((workers.len() as u64).to_be_bytes());
    for (name, bytes) in workers {
        put(
            &mut assets_digest,
            name.trim_start_matches("runtime/").as_bytes(),
        );
        put(&mut assets_digest, bytes);
    }
    let assets_hash = hex::encode(assets_digest.finalize());
    let out = PathBuf::from(env::var("OUT_DIR")?);
    // Copy exactly the bytes verified above; include_bytes must not reread mutable build inputs.
    fs::write(out.join("workerd.gz"), archive)?;
    let mut source = format!(
        "pub(super) const ARCHIVE: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/workerd.gz\"));\n\
         pub(super) const TARGET: &str = {target:?};\n\
         pub(super) const PAYLOAD_SHA256: &str = {payload_hash:?};\n\
         pub(super) const ASSETS_SHA256: &str = {assets_hash:?};\n\
         pub(super) const FILES: &[(&str, &[u8])] = &[\n"
    );
    for (index, (name, bytes)) in assets.iter().enumerate() {
        fs::write(out.join(format!("asset-{index}")), bytes)?;
        source.push_str(&format!(
            "({name:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/asset-{index}\"))),\n"
        ));
    }
    source.push_str("];\n");
    fs::write(out.join("embedded_payload.rs"), source)?;
    Ok(())
}

fn put(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("embedded build input exceeds its size bound".into());
    }
    Ok(bytes)
}

fn tracked(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    println!("cargo:rerun-if-changed={}", path.display());
    if !fs::symlink_metadata(path)?.is_file() {
        return Err("embedded asset must be a regular file, not a symlink".into());
    }
    read_bounded(path, 1024 * 1024)
}

fn collect(
    root: &Path,
    relative: &str,
    assets: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={}", root.join(relative).display());
    if !fs::symlink_metadata(root.join(relative))?.is_dir() {
        return Err("runtime asset/source directory must be a regular directory".into());
    }
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "asset path is not UTF-8")?;
        let path = format!("{relative}/{name}");
        if entry.file_type()?.is_dir() {
            collect(root, &path, assets)?;
        } else {
            assets.insert(path.clone(), tracked(&root.join(path))?);
        }
    }
    if assets.len() > 4096 || assets.values().map(Vec::len).sum::<usize>() > 8 * 1024 * 1024 {
        return Err("embedded assets exceed their size/count bounds".into());
    }
    Ok(())
}

fn verify_manifest(root: &Path, assets: &BTreeMap<String, Vec<u8>>) -> Result<(), Box<dyn Error>> {
    let manifest: serde_json::Value = serde_json::from_slice(
        assets
            .get("runtime/dist/manifest.json")
            .ok_or("runtime assets are missing; run bun run build before Cargo")?,
    )?;
    let mut inputs = BTreeMap::new();
    collect(root, "packages/runtime/src", &mut inputs)?;
    for name in [
        "bun.lock",
        "package.json",
        "tsconfig.json",
        "packages/runtime/build.ts",
        "packages/runtime/package.json",
        "packages/runtime/tsconfig.json",
        "packages/runtime/tsconfig.build.json",
    ] {
        inputs.insert(name.to_owned(), tracked(&root.join(name))?);
    }
    let expected_inputs = manifest["inputs"]
        .as_object()
        .ok_or("missing runtime build inputs")?;
    if inputs.len() != expected_inputs.len()
        || inputs.iter().any(|(name, bytes)| {
            expected_inputs
                .get(name)
                .and_then(serde_json::Value::as_str)
                != Some(hex::encode(Sha256::digest(bytes)).as_str())
        })
    {
        return Err(
            "runtime assets are stale for the current sources/toolchain; run bun run build".into(),
        );
    }
    let sources = manifest["sources"]
        .as_object()
        .ok_or("invalid runtime manifest")?;
    if manifest["schemaVersion"] != 1 || sources.len() + 3 != assets.len() {
        return Err("runtime manifest does not cover the exact embedded file set".into());
    }
    for (name, expected) in sources {
        let bytes = assets
            .get(&format!("runtime/dist/{name}"))
            .ok_or("runtime manifest source is missing")?;
        if expected.as_str() != Some(&hex::encode(Sha256::digest(bytes))) {
            return Err("generated runtime does not match its manifest; run bun run build".into());
        }
    }
    Ok(())
}
