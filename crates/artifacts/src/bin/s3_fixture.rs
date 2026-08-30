//! Long-lived `SigV4` network fixture for explicitly selected external conformance runs.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use open_compute_artifacts::MockS3;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = MockS3::spawn("open-compute").await;
    println!(
        "{{\"schemaVersion\":1,\"endpoint\":\"{}\"}}",
        fixture.endpoint
    );
    std::io::stdout().flush()?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}
