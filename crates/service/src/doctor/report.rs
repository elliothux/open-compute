use super::*;

mod platform;
mod storage;

/// Run doctor against a loaded config.
pub async fn doctor_report(
    loaded: &LoadedConfig,
    mode: DoctorMode,
    gateway: Option<&open_compute_core::PublicGatewayConfig>,
) -> DoctorReport {
    let mut checks = Vec::new();
    let (inspect, inspected_key, db_ok) = platform::inspect_platform(loaded, &mut checks);
    platform::inspect_scheduler(loaded, inspect.as_ref(), &mut checks);
    storage::inspect_local_components(loaded, &inspect, &inspected_key, &db_ok, &mut checks);
    let object_backend =
        storage::inspect_object_storage(loaded, mode, db_ok.as_ref(), &mut checks).await;
    storage::inspect_full(
        loaded,
        mode,
        inspect.as_ref(),
        db_ok.as_ref(),
        object_backend.as_ref(),
        &mut checks,
    )
    .await;
    inspect_gateway(loaded, gateway, &mut checks).await;

    checks.sort_by_key(|c| c.name);
    let result = if checks.iter().any(|c| c.status == CheckStatus::Failed) {
        "failed"
    } else {
        "ok"
    };
    DoctorReport {
        schema_version: 1,
        command: "doctor",
        result,
        checks,
    }
}

async fn inspect_gateway(
    loaded: &LoadedConfig,
    gateway: Option<&open_compute_core::PublicGatewayConfig>,
    checks: &mut Vec<DoctorCheck>,
) {
    let Some(domain) = &loaded.config.public_gateway else {
        return;
    };
    let Some(gateway) = gateway else {
        checks.push(failed(
            "gateway_config",
            ErrorCode::ConfigInvalid,
            "shared gateway settings are missing",
            Some(domain.base_domain.clone()),
        ));
        return;
    };
    checks.push(match gateway.validate() {
        Ok(()) => ok(
            "gateway_config",
            "public gateway intent is valid",
            Some(gateway.base_domain.clone()),
        ),
        Err(error) => failed(
            "gateway_config",
            error.code(),
            "public gateway intent is invalid",
            None,
        ),
    });
    checks.push(
        match serde_json::from_slice::<serde_json::Value>(
            open_compute_runtime::embedded_caddy_lock().unwrap_or_default(),
        ) {
            Ok(lock) => ok(
                "gateway_caddy_pin",
                "embedded Caddy pin is readable",
                lock.get("release")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            ),
            Err(_) => failed(
                "gateway_caddy_pin",
                ErrorCode::RuntimeInvalid,
                "embedded Caddy pin is invalid",
                None,
            ),
        },
    );
    checks.push(
        match crate::gateway_dns_verify::verify_public_gateway_dns(gateway, &[]).await {
            Ok(()) => ok(
                "gateway_dns",
                "public DNS and challenge delegation are valid",
                None,
            ),
            Err(error) => failed(
                "gateway_dns",
                error.code(),
                "public DNS or challenge delegation is invalid",
                None,
            ),
        },
    );
    checks.push(
        match crate::gateway_tls::probe_worker_gateway(
            gateway.shared.https_listen,
            &gateway.base_domain,
            std::time::Duration::from_secs(5),
        )
        .await
        {
            Ok(()) => ok(
                "gateway_tls",
                "managed Worker wildcard TLS route is ready",
                None,
            ),
            Err(error) => failed(
                "gateway_tls",
                error.code(),
                "managed Worker wildcard TLS route is unavailable",
                None,
            ),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn gateway_doctor_reports_pin_and_unavailable_public_endpoints() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = open_compute_core::PlatformConfig::local_test_config();
        let domain = open_compute_core::PublicDomainConfig {
            base_domain: "compute.invalid".to_owned(),
        };
        let shared = open_compute_core::DaemonGatewayConfig {
            ingress_ipv4: vec![Ipv4Addr::new(203, 0, 113, 10)],
            ingress_ipv6: Vec::new(),
            https_listen: "127.0.0.1:9".parse().unwrap(),
            challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
            proxy_protocol_from: Vec::new(),
            caddy: Vec::new(),
        };
        config.public_gateway = Some(domain.clone());
        let loaded = LoadedConfig {
            path: temp.path().join("open-compute.toml"),
            sha256: String::new(),
            config,
        };
        let mut checks = Vec::new();
        let gateway = open_compute_core::PublicGatewayConfig::resolve(&domain, &shared);
        inspect_gateway(&loaded, Some(&gateway), &mut checks).await;
        assert_eq!(checks.len(), 4);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert_eq!(checks[1].status, CheckStatus::Ok);
        assert_eq!(checks[2].status, CheckStatus::Failed);
        assert_eq!(checks[3].status, CheckStatus::Failed);
    }
}
