use super::*;

pub(super) fn require_operator_deps(
    deps: Option<&OperatorDeps>,
) -> Result<&OperatorDeps, PlatformError> {
    deps.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "operator dependencies were not initialized for this command",
        )
    })
}

pub(super) fn resolve_loaded_config(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: Option<&InstanceRegistry>,
) -> Result<LoadedConfig, PlatformError> {
    match (config, instance) {
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "--instance and --config are mutually exclusive",
        )),
        (None, Some(selector)) => {
            let registry = registry.ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::InstanceNotFound,
                    "instance registry is unavailable for --instance resolution",
                )
            })?;
            let record = registry.get(selector)?;
            load_platform_config_from(record.config_path(), startup_cwd)
        }
        (explicit, None) => discover_and_load_config(explicit, startup_cwd),
    }
}

pub(super) async fn interruptible_offline<T>(
    operation: impl Future<Output = Result<T, PlatformError>>,
) -> Result<T, PlatformError> {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        _ = async {
            match sigterm.as_mut() {
                Some(signal) => { signal.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => Err(offline_interrupted()),
        _ = async {
            match sigint.as_mut() {
                Some(signal) => { signal.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => Err(offline_interrupted()),
    }
}

pub(super) fn offline_interrupted() -> PlatformError {
    PlatformError::new(
        ErrorCode::PlatformUnavailable,
        "offline operation was interrupted before completion",
    )
}

pub(super) fn write_config_check(out: &mut impl Write, json: bool) -> Result<(), PlatformError> {
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "command": "config_check",
                "result": "ok",
            })
        )
        .map_err(|_| io_failed())?;
    } else {
        writeln!(out, "CONFIG_OK").map_err(|_| io_failed())?;
    }
    Ok(())
}

pub(super) fn write_gateway_dns_plan(
    out: &mut impl Write,
    gateway: &PublicGatewayConfig,
    json: bool,
) -> Result<(), PlatformError> {
    let records = gateway.dns_plan(false);
    if json {
        writeln!(
            out,
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "command": "config_gateway_dns_plan",
                "records": records,
                "inbound_ports": ["tcp/443", "udp/53", "tcp/53"],
                "https_listen": gateway.https_listen,
                "challenge_dns_listen": gateway.challenge_dns_listen,
            })
        )
        .map_err(|_| io_failed())?;
    } else {
        for record in records {
            let kind = match record.kind {
                GatewayDnsRecordKind::A => "A",
                GatewayDnsRecordKind::Aaaa => "AAAA",
                GatewayDnsRecordKind::Cname => "CNAME",
                GatewayDnsRecordKind::Ns => "NS",
            };
            writeln!(out, "{} {kind} {}", record.name, record.value).map_err(|_| io_failed())?;
        }
        writeln!(out, "Inbound: TCP 443, UDP 53, TCP 53").map_err(|_| io_failed())?;
        writeln!(out, "HTTPS bind: {}", gateway.https_listen).map_err(|_| io_failed())?;
        writeln!(out, "Challenge DNS bind: {}", gateway.challenge_dns_listen)
            .map_err(|_| io_failed())?;
    }
    Ok(())
}

pub(super) fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
}

pub(super) fn validate_setup_scope(is_root: bool, system: bool) -> Result<(), PlatformError> {
    if is_root && !system {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "root setup requires explicit system scope; retry with `ocd setup --system --yes`",
        ));
    }
    Ok(())
}

/// Load helper used by tests.
pub fn load_checked(path: &Path) -> Result<LoadedConfig, PlatformError> {
    let loaded = load_platform_config(path)?;
    MetricsRegistry::validate_limits(&loaded.config.metrics)?;
    Ok(loaded)
}
