//! One daemon-owned public Gateway, independent of instance lifetimes.

use super::*;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::AtomicI32;

pub(super) struct GatewayOwner {
    child_pid: Arc<AtomicI32>,
    qualified_pid: Arc<AtomicI32>,
    pub(super) authority: Arc<crate::challenge_dns::ChallengeAuthority>,
    pub(super) control: Arc<crate::gateway_control::GatewayControl>,
}

impl GatewayOwner {
    pub(super) fn pids(&self) -> (Arc<AtomicI32>, Arc<AtomicI32>) {
        (self.child_pid.clone(), self.qualified_pid.clone())
    }

    pub(super) fn update_domains(&self, domains: &[String]) -> Result<(), PlatformError> {
        self.control.validate_domains(domains)?;
        let previous = self.control.domains()?;
        let mut staged = previous.clone();
        for domain in domains {
            if !staged.contains(domain) {
                staged.push(domain.clone());
            }
        }
        let staged_zones = staged.iter().map(String::as_str).collect::<Vec<_>>();
        self.authority.replace_domains(&staged_zones)?;
        if let Err(error) = self.control.reload_domains(domains.to_vec()) {
            let previous_zones = previous.iter().map(String::as_str).collect::<Vec<_>>();
            self.authority.replace_domains(&previous_zones)?;
            return Err(error);
        }
        let active_zones = domains.iter().map(String::as_str).collect::<Vec<_>>();
        self.authority.replace_domains(&active_zones)?;
        Ok(())
    }
}

pub(super) async fn start(
    loaded_instances: &[LoadedConfig],
    opts: &RunInner,
    plan: Option<&DaemonPlan>,
    routes: &http::SharedRoutes,
    shutdown: watch::Receiver<bool>,
    listeners: &mut tokio::task::JoinSet<Result<(), PlatformError>>,
) -> Result<Option<GatewayOwner>, PlatformError> {
    let domains = if let Some(plan) = plan {
        plan.records
            .iter()
            .filter_map(|record| record.public_base_domain.clone())
            .collect::<Vec<_>>()
    } else {
        loaded_instances
            .iter()
            .filter_map(|loaded| {
                loaded
                    .config
                    .public_gateway
                    .as_ref()
                    .map(|config| config.base_domain.clone())
            })
            .collect::<Vec<_>>()
    };
    if domains.is_empty() && opts.daemon_gateway.is_none() {
        return Ok(None);
    }
    let shared = opts.daemon_gateway.as_ref().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "public domain requires a shared gateway configuration",
        )
    })?;
    let mut domains = domains;
    domains.sort_unstable();
    let root = match plan {
        Some(plan) => plan.root.as_path(),
        None => {
            let scope = opts
                .scope
                .ok_or_else(|| gateway_error("OCD scope is missing"))?;
            opts.instance_registry
                .as_ref()
                .ok_or_else(|| gateway_error("OCD registry is missing"))?
                .root_for(scope)
        }
    };
    prepare(
        root,
        shared.clone(),
        domains,
        opts.shared_package.clone(),
        routes,
        shutdown,
        listeners,
    )
    .await
    .map(Some)
}

#[allow(
    clippy::manual_let_else,
    reason = "test-only package materialization returns a value"
)]
async fn prepare(
    root: &Path,
    shared: DaemonGatewayConfig,
    domains: Vec<String>,
    package: Option<open_compute_runtime::RuntimePackage>,
    routes: &http::SharedRoutes,
    shutdown: watch::Receiver<bool>,
    listeners: &mut tokio::task::JoinSet<Result<(), PlatformError>>,
) -> Result<GatewayOwner, PlatformError> {
    let gateway_dir = root.join("gateway");
    for name in ["", "run", "config-state"] {
        open_compute_storage::ensure_dir_secure(&gateway_dir.join(name))?;
    }
    ensure_storage_identity(&gateway_dir)?;
    crate::gateway_certificates::check_certified_domains(&gateway_dir, &domains)?;
    open_compute_storage::ensure_dir_secure(&root.join("run"))?;
    let socket_dir = root.join("run/gateway");
    open_compute_storage::ensure_dir_secure(&socket_dir)?;
    let admin_path = socket_dir.join("admin.sock");
    let upstream_path = socket_dir.join("upstream.sock");
    let provider_path = socket_dir.join("dns.sock");
    crate::gateway_caddyfile::write_managed(
        &shared,
        &domains,
        &gateway_dir,
        &admin_path,
        &upstream_path,
        &provider_path,
    )?;
    let zones = domains.iter().map(String::as_str).collect::<Vec<_>>();
    let authority = Arc::new(crate::challenge_dns::ChallengeAuthority::new(
        &zones, false,
    )?);
    let challenge = crate::challenge_dns::ChallengeDnsServer::bind(
        shared.challenge_dns_listen,
        authority.clone(),
    )
    .await?;
    let child_pid = Arc::new(AtomicI32::new(0));
    let qualified_pid = Arc::new(AtomicI32::new(0));
    let provider = crate::challenge_dns::ChallengeProviderServer::bind(
        provider_path.clone(),
        authority.clone(),
        child_pid.clone(),
        gateway_dir.clone(),
    )?;
    let upstream = http::PrivateUnixListener::bind(upstream_path.clone())?;
    let control = Arc::new(crate::gateway_control::GatewayControl::new(
        shared.clone(),
        domains,
        gateway_dir.clone(),
        admin_path,
        upstream_path,
        provider_path,
        child_pid.clone(),
        qualified_pid.clone(),
    ));
    let package = match package {
        Some(package) => package,
        None => {
            #[cfg(any(test, feature = "test-support"))]
            {
                let cache_dir = root.join("cache");
                open_compute_storage::ensure_dir_secure(&cache_dir)?;
                tokio::task::spawn_blocking(move || {
                    open_compute_runtime::materialize_embedded_runtime(&cache_dir)
                })
                .await
                .map_err(|_| gateway_error("shared Gateway package materialization failed"))??
            }
            #[cfg(not(any(test, feature = "test-support")))]
            {
                return Err(gateway_error("shared runtime package is unavailable"));
            }
        }
    };
    let process = crate::gateway_process::GatewayProcess {
        package,
        shared,
        gateway_dir,
        child_pid: child_pid.clone(),
        qualified_pid: qualified_pid.clone(),
        redactor: Redactor::new(),
        control: control.clone(),
    };
    let router = routes.gateway_router();
    let caddy_pid = child_pid.clone();
    let mut upstream_shutdown = shutdown.clone();
    listeners.spawn(async move {
        upstream
            .serve_gateway(router, caddy_pid, async move {
                let _ = upstream_shutdown.changed().await;
            })
            .await
    });
    listeners.spawn(challenge.serve(shutdown.clone()));
    listeners.spawn(provider.serve(shutdown.clone()));
    listeners.spawn(process.run(shutdown));
    Ok(GatewayOwner {
        child_pid,
        qualified_pid,
        authority,
        control,
    })
}

fn gateway_error(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, message)
}

fn ensure_storage_identity(gateway_dir: &Path) -> Result<(), PlatformError> {
    let storage = gateway_dir.join("storage");
    let outside = gateway_dir.join("config-state/storage.id");
    let inside = storage.join(".ocd-storage-id");
    let storage_present = path_exists(&storage)?;
    let outside_present = path_exists(&outside)?;
    let inside_present = if storage_present {
        open_compute_storage::ensure_dir_secure(&storage)?;
        path_exists(&inside)?
    } else {
        false
    };
    match (outside_present, inside_present) {
        (true, true) => {
            if read_storage_id(&outside)? != read_storage_id(&inside)? {
                return Err(gateway_error(
                    "Gateway ACME storage identity does not match",
                ));
            }
            crate::gateway_certificates::check_registry(gateway_dir)?;
            crate::gateway_certificates::check_present_certificates(&storage)?;
        }
        (false, false)
            if !storage_present
                && !path_exists(&gateway_dir.join("Caddyfile"))?
                && !path_exists(&gateway_dir.join("managed.caddyfile"))?
                && !path_exists(&gateway_dir.join("config-state/current.meta.json"))? =>
        {
            open_compute_storage::ensure_dir_secure(&storage)?;
            crate::gateway_certificates::initialize_registry(gateway_dir)?;
            let id = uuid::Uuid::now_v7().as_simple().to_string();
            open_compute_storage::atomic_write(&inside, id.as_bytes())?;
            open_compute_storage::atomic_write(&outside, id.as_bytes())?;
        }
        _ => {
            return Err(gateway_error(
                "Gateway ACME storage is missing or incomplete",
            ));
        }
    }
    Ok(())
}

fn path_exists(path: &Path) -> Result<bool, PlatformError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(gateway_error("Gateway state could not be inspected")),
    }
}

fn read_storage_id(path: &Path) -> Result<String, PlatformError> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| gateway_error("Gateway ACME storage identity is invalid"))?;
    let file = fs::File::from(fd);
    let meta = file
        .metadata()
        .map_err(|_| gateway_error("Gateway ACME storage identity is invalid"))?;
    if !meta.is_file()
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.permissions().mode() & 0o777 != 0o600
    {
        return Err(gateway_error("Gateway ACME storage identity is invalid"));
    }
    let mut value = String::new();
    file.take(33)
        .read_to_string(&mut value)
        .map_err(|_| gateway_error("Gateway ACME storage identity is invalid"))?;
    let id = uuid::Uuid::parse_str(&value)
        .map_err(|_| gateway_error("Gateway ACME storage identity is invalid"))?;
    if value.len() != 32 || id.get_version() != Some(uuid::Version::SortRand) {
        return Err(gateway_error("Gateway ACME storage identity is invalid"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::Ordering;

    #[test]
    fn acme_storage_identity_distinguishes_first_start_from_state_loss() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = temp.path().join("gateway");
        open_compute_storage::ensure_dir_secure(&gateway).unwrap();
        open_compute_storage::ensure_dir_secure(&gateway.join("config-state")).unwrap();
        ensure_storage_identity(&gateway).unwrap();
        let outside = fs::read(gateway.join("config-state/storage.id")).unwrap();
        assert_eq!(
            outside,
            fs::read(gateway.join("storage/.ocd-storage-id")).unwrap()
        );
        ensure_storage_identity(&gateway).unwrap();
        fs::remove_dir_all(gateway.join("storage")).unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        assert!(!gateway.join("storage").exists());
        assert_eq!(
            fs::read(gateway.join("config-state/storage.id")).unwrap(),
            outside
        );
    }

    #[test]
    fn acme_storage_identity_rejects_mismatch_and_old_unmarked_state() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = temp.path().join("gateway");
        open_compute_storage::ensure_dir_secure(&gateway).unwrap();
        open_compute_storage::ensure_dir_secure(&gateway.join("config-state")).unwrap();
        ensure_storage_identity(&gateway).unwrap();
        fs::write(
            gateway.join("storage/.ocd-storage-id"),
            uuid::Uuid::now_v7().as_simple().to_string(),
        )
        .unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::write(gateway.join("storage/.ocd-storage-id"), b"corrupt").unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::set_permissions(
            gateway.join("storage/.ocd-storage-id"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::remove_file(gateway.join("storage/.ocd-storage-id")).unwrap();
        symlink(
            gateway.join("config-state/storage.id"),
            gateway.join("storage/.ocd-storage-id"),
        )
        .unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::remove_file(gateway.join("storage/.ocd-storage-id")).unwrap();
        fs::remove_file(gateway.join("config-state/storage.id")).unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }

    #[test]
    fn acme_storage_rejects_one_missing_certificate_asset() {
        let temp = tempfile::tempdir().unwrap();
        let gateway = temp.path().join("gateway");
        open_compute_storage::ensure_dir_secure(&gateway).unwrap();
        open_compute_storage::ensure_dir_secure(&gateway.join("config-state")).unwrap();
        ensure_storage_identity(&gateway).unwrap();
        let certificates = gateway.join("storage/certificates");
        let issuer = certificates.join("issuer");
        let site = issuer.join("wildcard_example.com");
        for path in [&certificates, &issuer, &site] {
            open_compute_storage::ensure_dir_secure(path).unwrap();
        }
        for suffix in ["crt", "key", "json"] {
            open_compute_storage::atomic_write(
                &site.join(format!("wildcard_example.com.{suffix}")),
                b"asset",
            )
            .unwrap();
        }
        ensure_storage_identity(&gateway).unwrap();
        for suffix in ["crt", "key", "json"] {
            let path = site.join(format!("wildcard_example.com.{suffix}"));
            fs::remove_file(&path).unwrap();
            assert_eq!(
                ensure_storage_identity(&gateway).unwrap_err().code(),
                ErrorCode::ConfigInvalid
            );
            open_compute_storage::atomic_write(&path, b"asset").unwrap();
        }
        fs::set_permissions(&site, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
        fs::set_permissions(&site, fs::Permissions::from_mode(0o700)).unwrap();
        let key = site.join("wildcard_example.com.key");
        fs::remove_file(&key).unwrap();
        symlink(site.join("wildcard_example.com.crt"), &key).unwrap();
        assert_eq!(
            ensure_storage_identity(&gateway).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }

    #[test]
    fn rejected_reload_restores_the_previous_challenge_authority() {
        let temp = tempfile::tempdir().unwrap();
        let gateway_dir = temp.path().join("gateway");
        open_compute_storage::ensure_dir_secure(&gateway_dir).unwrap();
        open_compute_storage::ensure_dir_secure(&gateway_dir.join("config-state")).unwrap();
        let previous = "old.example.com";
        let next = "new.example.net";
        let authority =
            Arc::new(crate::challenge_dns::ChallengeAuthority::new(&[previous], false).unwrap());
        let child_pid = Arc::new(AtomicI32::new(0));
        let qualified_pid = Arc::new(AtomicI32::new(0));
        let control = Arc::new(crate::gateway_control::GatewayControl::new(
            DaemonGatewayConfig {
                ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
                ingress_ipv6: Vec::new(),
                https_listen: "127.0.0.1:8443".parse().unwrap(),
                challenge_dns_listen: "127.0.0.1:8053".parse().unwrap(),
                proxy_protocol_from: Vec::new(),
                caddy: Vec::new(),
            },
            vec![previous.to_owned()],
            gateway_dir,
            temp.path().join("admin.sock"),
            temp.path().join("upstream.sock"),
            temp.path().join("dns.sock"),
            child_pid.clone(),
            qualified_pid.clone(),
        ));
        let gateway = GatewayOwner {
            child_pid,
            qualified_pid,
            authority,
            control,
        };
        gateway
            .authority
            .append("_acme-challenge.old.example.com", "pending-token")
            .unwrap();
        assert!(gateway.update_domains(&[next.to_owned()]).is_err());
        assert_eq!(gateway.control.domains().unwrap(), vec![previous]);
        assert!(
            gateway
                .authority
                .delete_exact("_acme-challenge.old.example.com", "pending-token")
                .unwrap()
        );
        assert!(
            gateway
                .authority
                .append("_acme-challenge.old.example.com", "old-token")
                .is_ok()
        );
        assert!(
            gateway
                .authority
                .append("_acme-challenge.new.example.net", "new-token")
                .is_err()
        );
    }

    #[tokio::test]
    async fn empty_shared_gateway_accepts_first_domain_and_later_removes_it() {
        let temp = tempfile::Builder::new()
            .prefix("oc-gw-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = temp.path().join("user");
        open_compute_storage::ensure_dir_secure(&root).unwrap();
        let _lock = DaemonLock::acquire(&root).unwrap();
        let https = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let https_listen = https.local_addr().unwrap();
        drop(https);
        let challenge = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let challenge_dns_listen = challenge.local_addr().unwrap();
        let udp = std::net::UdpSocket::bind(challenge_dns_listen).unwrap();
        drop((challenge, udp));
        let registry = InstanceRegistry::with_roots(temp.path().join("system"), root.clone());
        let opts = RunInner {
            daemon_gateway: Some(DaemonGatewayConfig {
                ingress_ipv4: vec!["203.0.113.10".parse().unwrap()],
                ingress_ipv6: Vec::new(),
                https_listen,
                challenge_dns_listen,
                proxy_protocol_from: Vec::new(),
                caddy: Vec::new(),
            }),
            scope: Some(ServiceScope::User),
            instance_registry: Some(registry),
            ..RunInner::default()
        };
        let routes = http::SharedRoutes::new(None, 1024);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut listeners = tokio::task::JoinSet::new();
        let gateway = start(&[], &opts, None, &routes, shutdown_rx, &mut listeners)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if gateway.child_pid.load(Ordering::Acquire) > 0
                    && std::os::unix::net::UnixStream::connect(root.join("run/gateway/admin.sock"))
                        .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        gateway
            .update_domains(&["first.example.com".to_owned()])
            .unwrap();
        assert!(
            fs::read_to_string(root.join("gateway/managed.caddyfile"))
                .unwrap()
                .contains("https://*.first.example.com")
        );
        gateway.update_domains(&[]).unwrap();
        assert!(
            !fs::read_to_string(root.join("gateway/managed.caddyfile"))
                .unwrap()
                .contains("first.example.com")
        );
        shutdown_tx.send(true).unwrap();
        while let Some(task) = listeners.join_next().await {
            task.unwrap().unwrap();
        }
        assert_eq!(
            fs::read(root.join("gateway/storage/.ocd-storage-id")).unwrap(),
            fs::read(root.join("gateway/config-state/storage.id")).unwrap()
        );
    }
}
