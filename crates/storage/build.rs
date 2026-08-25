//! Emit compile-time SHA-256 checksums for versioned migration SQL files.

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let migrations = [
        ("001_init", "MIGRATION_001_SHA256"),
        ("002_workers_runtime", "MIGRATION_002_SHA256"),
        ("003_resource_bindings", "MIGRATION_003_SHA256"),
        ("004_kv", "MIGRATION_004_SHA256"),
        ("005_r2", "MIGRATION_005_SHA256"),
        ("006_d1", "MIGRATION_006_SHA256"),
        ("007_durable_objects", "MIGRATION_007_SHA256"),
    ];
    let mut generated = String::new();
    for (file, constant) in migrations {
        let sql_path = manifest_dir.join("migrations").join(format!("{file}.sql"));
        println!("cargo:rerun-if-changed={}", sql_path.display());
        let sql = fs::read(&sql_path).unwrap_or_else(|_| panic!("read migration {file}"));
        let digest = Sha256::digest(&sql);
        let mut literal = String::from("[");
        for (i, byte) in digest.iter().enumerate() {
            if i > 0 {
                literal.push_str(", ");
            }
            literal.push_str(&format!("0x{byte:02x}"));
        }
        literal.push(']');
        generated.push_str(&format!(
            "/// SHA-256 of `{file}.sql` captured at build time.\npub const {constant}: [u8; 32] = {literal};\n"
        ));
    }
    let scheduler_file = "001_scheduler";
    let scheduler_path = manifest_dir
        .join("scheduler-migrations")
        .join(format!("{scheduler_file}.sql"));
    println!("cargo:rerun-if-changed={}", scheduler_path.display());
    let scheduler_sql = fs::read(&scheduler_path).expect("read scheduler migration");
    let digest = Sha256::digest(&scheduler_sql);
    let literal = digest
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(", ");
    generated.push_str(&format!(
        "/// SHA-256 of `{scheduler_file}.sql` captured at build time.\n\
         pub const SCHEDULER_MIGRATION_001_SHA256: [u8; 32] = [{literal}];\n"
    ));

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("migration_hashes.rs");
    fs::write(out, generated).expect("write migration hashes");
}
