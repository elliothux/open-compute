//! Human and JSON command boundary for the remote target registry.

use crate::target_http::{TargetHttp, probe_target};
use crate::target_registry::{TargetRecord, TargetRegistry, read_target_token};
use open_compute_core::{
    CloudflareAccountId, ErrorCode, PlatformError, TargetApiBaseUrl, TargetName,
};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

/// Add one target and print its secret-free descriptor.
pub fn add_target(
    registry: &TargetRegistry,
    name: TargetName,
    api_base_url: TargetApiBaseUrl,
    account_id: CloudflareAccountId,
    token_file: PathBuf,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = registry.add(
        name,
        api_base_url,
        account_id,
        token_file,
        SystemTime::now(),
    )?;
    writeln!(
        out,
        "TARGET_ADDED {} {} {}",
        record.name,
        record.api_base_url.origin(),
        record.account_id
    )
    .map_err(|_| io_failed())
}

/// List registered targets without reading any token file.
pub fn list_targets(
    registry: &TargetRegistry,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let records = registry.list()?;
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "target_list",
            "targets": records.iter().map(target_json).collect::<Vec<_>>(),
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else if records.is_empty() {
        writeln!(out, "No registered targets.").map_err(|_| io_failed())?;
    } else {
        writeln!(out, "NAME  ORIGIN  ACCOUNT  TOKEN_FILE").map_err(|_| io_failed())?;
        for record in records {
            writeln!(
                out,
                "{}  {}  {}  {}",
                record.name,
                record.api_base_url.origin(),
                record.account_id,
                record.token_file.display()
            )
            .map_err(|_| io_failed())?;
        }
    }
    Ok(())
}

/// Show one target without opening its token file.
pub fn show_target(
    registry: &TargetRegistry,
    name: &TargetName,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let record = registry.get(name)?;
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "target_show",
            "target": target_json(&record),
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else {
        writeln!(out, "TARGET {}", record.name).map_err(|_| io_failed())?;
        writeln!(out, "api_base_url={}", record.api_base_url).map_err(|_| io_failed())?;
        writeln!(out, "account_id={}", record.account_id).map_err(|_| io_failed())?;
        writeln!(out, "token_file={}", record.token_file.display()).map_err(|_| io_failed())?;
    }
    Ok(())
}

/// Probe authentication, account discovery, and the certified Wrangler pin.
pub async fn test_target(
    registry: &TargetRegistry,
    http: &dyn TargetHttp,
    name: &TargetName,
    out: &mut impl Write,
    json: bool,
) -> Result<(), PlatformError> {
    let record = registry.get(name)?;
    let token = read_target_token(&record.token_file)?;
    let capabilities = probe_target(http, &record, &token).await?;
    if json {
        let payload = serde_json::json!({
            "schema_version": 1,
            "command": "target_test",
            "result": "ok",
            "target": record.name,
            "origin": record.api_base_url.origin(),
            "account_id": record.account_id,
            "wrangler_version": capabilities.wrangler_version,
        });
        writeln!(out, "{payload}").map_err(|_| io_failed())?;
    } else {
        writeln!(
            out,
            "TARGET_OK {} {} {} wrangler={}",
            record.name,
            record.api_base_url.origin(),
            record.account_id,
            capabilities.wrangler_version
        )
        .map_err(|_| io_failed())?;
    }
    Ok(())
}

/// Remove one target record without deleting the referenced token file.
pub fn remove_target(
    registry: &TargetRegistry,
    name: &TargetName,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let removed = registry.remove(name)?;
    writeln!(out, "TARGET_REMOVED {}", removed.name).map_err(|_| io_failed())
}

fn target_json(record: &TargetRecord) -> serde_json::Value {
    serde_json::json!({
        "name": record.name,
        "api_base_url": record.api_base_url,
        "account_id": record.account_id,
        "token_file": record.token_file,
        "created_at": record.created_at,
    })
}

fn io_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "failed to write target command output",
    )
}
