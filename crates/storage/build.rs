//! Emit compile-time SHA-256 checksums for versioned migration SQL files.

use sha2::{Digest, Sha256};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let migrations = [
        ("migrations", "001_init", "MIGRATION_001_SHA256"),
        ("migrations", "002_workers_runtime", "MIGRATION_002_SHA256"),
        (
            "migrations",
            "003_resource_bindings",
            "MIGRATION_003_SHA256",
        ),
        ("migrations", "004_kv", "MIGRATION_004_SHA256"),
        ("migrations", "005_r2", "MIGRATION_005_SHA256"),
        ("migrations", "006_d1", "MIGRATION_006_SHA256"),
        ("migrations", "007_durable_objects", "MIGRATION_007_SHA256"),
        ("migrations", "008_queues", "MIGRATION_008_SHA256"),
        ("migrations", "009_queue_consumers", "MIGRATION_009_SHA256"),
        ("migrations", "010_cron_triggers", "MIGRATION_010_SHA256"),
        ("migrations", "011_workflows", "MIGRATION_011_SHA256"),
        ("migrations", "012_static_assets", "MIGRATION_012_SHA256"),
        ("migrations", "013_service_bindings", "MIGRATION_013_SHA256"),
        ("migrations", "014_cache_images", "MIGRATION_014_SHA256"),
        ("migrations", "015_vectorize", "MIGRATION_015_SHA256"),
        ("migrations", "016_ai_search", "MIGRATION_016_SHA256"),
        (
            "migrations",
            "017_system_owned_workers",
            "MIGRATION_017_SHA256",
        ),
        (
            "migrations",
            "018_cloudflare_artifacts",
            "MIGRATION_018_SHA256",
        ),
        (
            "migrations",
            "019_ai_search_r2_sources",
            "MIGRATION_019_SHA256",
        ),
        (
            "scheduler-migrations",
            "001_scheduler",
            "SCHEDULER_MIGRATION_001_SHA256",
        ),
        (
            "scheduler-migrations",
            "002_queue_producer",
            "SCHEDULER_MIGRATION_002_SHA256",
        ),
        (
            "scheduler-migrations",
            "003_queue_consumer",
            "SCHEDULER_MIGRATION_003_SHA256",
        ),
        (
            "scheduler-migrations",
            "004_cron",
            "SCHEDULER_MIGRATION_004_SHA256",
        ),
        (
            "scheduler-migrations",
            "005_workflow",
            "SCHEDULER_MIGRATION_005_SHA256",
        ),
        (
            "observability-migrations",
            "001_observability",
            "OBSERVABILITY_MIGRATION_001_SHA256",
        ),
    ];
    let mut generated = String::new();
    for (directory, file, constant) in migrations {
        let sql_path = manifest_dir.join(directory).join(format!("{file}.sql"));
        println!("cargo:rerun-if-changed={}", sql_path.display());
        let sql = fs::read(&sql_path)?;
        let digest = Sha256::digest(&sql);
        let literal = digest
            .iter()
            .map(|byte| format!("0x{byte:02x}"))
            .collect::<Vec<_>>()
            .join(", ");
        generated.push_str(&format!(
            "/// SHA-256 of `{file}.sql` captured at build time.\npub const {constant}: [u8; 32] = [{literal}];\n"
        ));
    }
    let out = PathBuf::from(env::var("OUT_DIR")?).join("migration_hashes.rs");
    fs::write(out, generated)?;
    Ok(())
}
