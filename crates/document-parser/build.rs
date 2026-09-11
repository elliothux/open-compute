//! Verifies and deterministically embeds the fixed OCR language assets.

use flate2::{Compression, GzBuilder};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TessdataLock {
    schema_version: u8,
    revision: String,
    license: String,
    language_expression: String,
    assets: Vec<TessdataAsset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TessdataAsset {
    language: String,
    url: String,
    size: u64,
    sha256: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=tessdata.lock.json");
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| std::io::Error::other("CARGO_MANIFEST_DIR is missing"))?,
    );
    let lock_path = manifest.join("tessdata.lock.json");
    let lock_bytes = fs::read(&lock_path)?;
    let lock: TessdataLock = serde_json::from_slice(&lock_bytes)?;
    assert_eq!(lock.schema_version, 1, "unsupported tessdata lock schema");
    assert_eq!(lock.revision, "87416418657359cb625c412a48b6e1d6d41c29bd");
    assert_eq!(lock.license, "Apache-2.0");
    assert_eq!(lock.language_expression, "eng+chi_sim+chi_tra");
    let expected = ["eng", "chi_sim", "chi_tra"];
    assert_eq!(
        lock.assets.len(),
        expected.len(),
        "unexpected tessdata asset count"
    );
    let source_root = manifest.join("../../share/tessdata");
    let output_root = PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or_else(|| std::io::Error::other("OUT_DIR is missing"))?,
    );
    for (asset, language) in lock.assets.iter().zip(expected) {
        assert_eq!(asset.language, language, "tessdata order drift");
        assert!(asset.url.ends_with(&format!("/{language}.traineddata")));
        let source = source_root.join(format!("{language}.traineddata"));
        println!("cargo:rerun-if-changed={}", source.display());
        verify(&source, asset)?;
        compress(
            &source,
            &output_root.join(format!("{language}.traineddata.gz")),
        )?;
    }
    println!(
        "cargo:rustc-env=OPEN_COMPUTE_TESSDATA_LOCK_SHA256={}",
        hex(&Sha256::digest(lock_bytes))
    );
    Ok(())
}

fn verify(path: &Path, asset: &TessdataAsset) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    assert_eq!(
        u64::try_from(bytes.len())?,
        asset.size,
        "tessdata size mismatch"
    );
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        asset.sha256,
        "tessdata digest mismatch"
    );
    Ok(())
}

fn compress(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(source)?;
    let file = fs::File::create(destination)?;
    let mut encoder = GzBuilder::new().mtime(0).write(file, Compression::best());
    encoder.write_all(&bytes)?;
    let _ = encoder.finish()?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
