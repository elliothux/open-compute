//! Per-instance task ownership within one scoped daemon.

use super::*;

pub(super) struct DaemonPlan {
    pub(super) root: std::path::PathBuf,
    pub(super) scope: ServiceScope,
    pub(super) registry: InstanceRegistry,
    pub(super) manifest_digest: Option<String>,
    pub(super) records: Vec<InstanceRecord>,
    pub(super) credentials: Vec<daemon_control::RegisteredTokens>,
}

pub(super) type RuntimeTasks =
    tokio::task::JoinSet<(Option<InstanceId>, Result<(), PlatformError>)>;

pub(super) async fn clean_stopped_cache(
    plan: &DaemonPlan,
    id: &InstanceId,
    dry_run: bool,
) -> Result<open_compute_artifacts::CacheCleanReport, PlatformError> {
    plan.registry.require_unchanged_online(
        plan.scope,
        &plan.records,
        plan.manifest_digest.as_deref(),
    )?;
    let record = plan
        .records
        .iter()
        .find(|record| record.instance_id == id.as_str())
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
        })?;
    clean_registered_instance_cache(&plan.root, record, dry_run).await
}

pub(crate) async fn clean_registered_instance_cache(
    root: &std::path::Path,
    record: &InstanceRecord,
    dry_run: bool,
) -> Result<open_compute_artifacts::CacheCleanReport, PlatformError> {
    OfflineInstanceOwner::acquire(root, record, dry_run)?
        .clean(dry_run)
        .await
}

pub(crate) struct OfflineInstanceOwner {
    cache_path: std::path::PathBuf,
    _readonly: Option<open_compute_storage::InspectLock>,
    _owned: Option<open_compute_storage::DataDir>,
}

impl OfflineInstanceOwner {
    pub(crate) fn acquire(
        root: &std::path::Path,
        record: &InstanceRecord,
        dry_run: bool,
    ) -> Result<Self, PlatformError> {
        let loaded = crate::config_load::load_platform_config_from(
            record.config_path(),
            std::path::Path::new("/"),
        )?;
        let data = &loaded.config.data;
        if crate::instance_registry::validate_instance_data_path(root, &data.path)?
            != std::path::Path::new(&record.data_path)
        {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered instance data path changed",
            ));
        }
        let readonly = if dry_run {
            open_compute_storage::fs::validate_root(&data.path)?;
            Some(
                open_compute_storage::InspectLock::try_acquire(&data.data_lock_path())?
                    .ok_or_else(|| {
                        PlatformError::new(
                            ErrorCode::DataDirInUse,
                            "instance data directory is in use",
                        )
                    })?,
            )
        } else {
            None
        };
        let owned = if dry_run {
            None
        } else {
            Some(open_compute_storage::DataDir::acquire_existing_offline(
                data,
            )?)
        };
        let (_, identity) = open_compute_storage::inspect_control_db(
            &data.path.join("control.sqlite"),
            data.sqlite_busy_timeout_ms,
        )?;
        if identity.instance_id != record.instance_id()? {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered instance identity changed",
            ));
        }
        let (lock, _) = open_compute_runtime::embedded_runtime_lock()?;
        let (_, target) = lock.current_target()?;
        let lease = data.path.join("runtime/child.lease");
        if dry_run {
            open_compute_runtime::assert_no_live_orphan(&lease, &target.binary_sha256)?;
            inspect_extension_orphans(&data.path, &target.binary_sha256)?;
        } else {
            open_compute_runtime::PersistentHostProcess::recover_orphan(
                &lease,
                &target.binary_sha256,
            )?;
            if let Some(owner) = &owned {
                for (_, path) in owner.existing_extension_provider_dirs()? {
                    open_compute_runtime::PersistentHostProcess::recover_recorded_orphan(
                        &path.join("provider.lease"),
                    )?;
                }
            }
        }
        Ok(Self {
            cache_path: data.path.join("cache/artifacts"),
            _readonly: readonly,
            _owned: owned,
        })
    }

    pub(crate) async fn clean(
        &self,
        dry_run: bool,
    ) -> Result<open_compute_artifacts::CacheCleanReport, PlatformError> {
        let cache = match std::fs::symlink_metadata(&self.cache_path) {
            Ok(_) => ArtifactCache::inspect_existing(self.cache_path.clone())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(open_compute_artifacts::CacheCleanReport::default());
            }
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "instance artifact cache is inaccessible",
                ));
            }
        };
        cache.clean(dry_run).await
    }
}

fn inspect_extension_orphans(
    data_path: &std::path::Path,
    digest: &str,
) -> Result<(), PlatformError> {
    let parent = data_path.join("runtime/extensions");
    match std::fs::symlink_metadata(&parent) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "extension process directory is inaccessible",
            ));
        }
        Ok(_) => {
            let _ = open_compute_runtime::open_host_directory_nofollow(&parent)?;
        }
    }
    for entry in std::fs::read_dir(&parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "extension process directory is inaccessible",
        )
    })? {
        let entry = entry.map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "extension process entry is inaccessible",
            )
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "extension process name is invalid")
        })?;
        open_compute_core::validate_local_extension_name(&name)?;
        let path = parent.join(name);
        let _ = open_compute_runtime::open_host_directory_nofollow(&path)?;
        open_compute_runtime::assert_no_live_orphan(&path.join("provider.lease"), digest)?;
    }
    Ok(())
}

pub(crate) fn validate_registered_tokens(
    records: &[InstanceRecord],
    server: &DaemonServerConfig,
) -> Result<Vec<daemon_control::RegisteredTokens>, PlatformError> {
    let admin = crate::auth::resolve_admin_auth(&server.admin_auth)?;
    let mut credentials: Vec<daemon_control::RegisteredTokens> = Vec::with_capacity(records.len());
    for record in records {
        let loaded = crate::config_load::load_platform_config_from(
            record.config_path(),
            std::path::Path::new("/"),
        )?;
        let deployer = crate::auth::resolve_bearer_auth(&loaded.config.auth.deployer_auth)?;
        let read_only = crate::auth::resolve_bearer_auth(&loaded.config.auth.read_only_auth)?;
        let conflict = deployer.expose() == read_only.expose()
            || [deployer.expose(), read_only.expose()]
                .into_iter()
                .any(|value| {
                    value == admin.expose()
                        || credentials.iter().any(|entry| {
                            value == entry.deployer.expose() || value == entry.read_only.expose()
                        })
                });
        if conflict {
            return Err(PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "registered instance Bearer tokens conflict",
            ));
        }
        credentials.push(daemon_control::RegisteredTokens {
            instance_id: record.instance_id()?,
            deployer,
            read_only,
        });
    }
    Ok(credentials)
}

pub(super) fn spawn_runtime(
    loaded: LoadedConfig,
    id: Option<InstanceId>,
    opts: &RunInner,
    instances: &mut RuntimeTasks,
    active: &mut HashMap<InstanceId, watch::Sender<bool>>,
    shutdowns: &mut Vec<watch::Sender<bool>>,
    daemon: Option<&daemon_control::DaemonApi>,
) -> Result<(), PlatformError> {
    if let Some(id) = &id {
        if active.contains_key(id) {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance runtime is already active",
            ));
        }
        if let Some(daemon) = daemon {
            daemon.mark(id, "starting", None)?;
        }
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut instance_opts = opts.clone();
    instance_opts.shutdown = Some(shutdown_rx);
    if let Some(id) = &id {
        active.insert(*id, shutdown_tx.clone());
    }
    shutdowns.push(shutdown_tx);
    instances.spawn(async move { (id, run_inner(loaded, instance_opts).await) });
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "daemon command coordinates distinct owners"
)]
pub(super) fn handle_daemon_command(
    command: &daemon_control::DaemonCommand,
    plan: Option<&mut DaemonPlan>,
    gateway: Option<&gateway::GatewayOwner>,
    opts: &RunInner,
    instances: &mut RuntimeTasks,
    active: &mut HashMap<InstanceId, watch::Sender<bool>>,
    shutdowns: &mut Vec<watch::Sender<bool>>,
    daemon: Option<&daemon_control::DaemonApi>,
) -> Result<(), PlatformError> {
    let Some(plan) = plan else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "daemon lifecycle authority is unavailable",
        ));
    };
    match &command.request {
        daemon_control::ControlRequest::List => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "list is not a lifecycle command",
        )),
        daemon_control::ControlRequest::CaddyStatus
        | daemon_control::ControlRequest::CaddyReload
        | daemon_control::ControlRequest::CaddyValidate => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "Caddy operation is not a lifecycle command",
        )),
        daemon_control::ControlRequest::CleanCache { .. }
        | daemon_control::ControlRequest::CleanGlobalCache { .. } => Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "cache clean is not a lifecycle command",
        )),
        daemon_control::ControlRequest::Add { config_path } => attach_instance(
            plan,
            config_path,
            true,
            true,
            gateway,
            opts,
            instances,
            active,
            shutdowns,
            daemon,
        ),
        daemon_control::ControlRequest::Create {
            config_path,
            data_dir,
            name,
            autostart,
            start,
        } => {
            plan.registry.require_unchanged_online(
                plan.scope,
                &plan.records,
                plan.manifest_digest.as_deref(),
            )?;
            crate::setup::create_instance(
                &plan.root,
                plan.scope,
                config_path,
                data_dir,
                name.as_ref(),
            )?;
            attach_instance(
                plan,
                config_path,
                *autostart,
                *start,
                gateway,
                opts,
                instances,
                active,
                shutdowns,
                daemon,
            )
        }
        daemon_control::ControlRequest::Remove { instance_id } => {
            if active.contains_key(instance_id) {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance is still running",
                ));
            }
            let record = plan
                .records
                .iter()
                .find(|record| record.instance_id == instance_id.as_str())
                .ok_or_else(|| {
                    PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
                })?;
            plan.manifest_digest = plan.registry.remove_online(
                record,
                &plan.records,
                plan.manifest_digest.as_deref(),
            )?;
            plan.records
                .retain(|record| record.instance_id != instance_id.as_str());
            if let Some(api) = daemon {
                api.remove(instance_id)?;
            }
            update_gateway_domains(plan, gateway)?;
            Ok(())
        }
        daemon_control::ControlRequest::Start { instance_id } => {
            let id = instance_id;
            if active.contains_key(id) {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance runtime is already active",
                ));
            }
            let api = daemon.ok_or_else(|| {
                PlatformError::new(ErrorCode::RuntimeUnavailable, "daemon API is unavailable")
            })?;
            let loaded = refresh_stopped_instance(plan, id, gateway, opts, api)?;
            spawn_runtime(
                loaded,
                Some(*id),
                opts,
                instances,
                active,
                shutdowns,
                daemon,
            )
        }
        daemon_control::ControlRequest::Stop { instance_id } => {
            let id = instance_id;
            if !plan
                .records
                .iter()
                .any(|record| record.instance_id == id.as_str())
            {
                return Err(PlatformError::new(
                    ErrorCode::InstanceNotFound,
                    "instance is not registered",
                ));
            }
            let shutdown = active.get(id).ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance is not running",
                )
            })?;
            if let Some(daemon) = daemon {
                daemon.mark(id, "stopping", None)?;
            }
            shutdown.send(true).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "instance runtime has stopped",
                )
            })
        }
    }
}

fn refresh_stopped_instance(
    plan: &mut DaemonPlan,
    id: &InstanceId,
    gateway: Option<&gateway::GatewayOwner>,
    opts: &RunInner,
    api: &daemon_control::DaemonApi,
) -> Result<LoadedConfig, PlatformError> {
    let old = plan
        .records
        .iter()
        .find(|record| record.instance_id == id.as_str())
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
        })?;
    let refreshed = plan.registry.list_scope(plan.scope)?;
    if refreshed.len() != plan.records.len()
        || refreshed.iter().any(|record| {
            if record.instance_id == id.as_str() {
                record.config_path() != old.config_path() || record.autostart != old.autostart
            } else {
                !plan.records.contains(record)
            }
        })
    {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "registered instance manifest or another config changed outside the daemon",
        ));
    }
    let record = refreshed
        .iter()
        .find(|record| record.instance_id == id.as_str())
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered identity changed",
            )
        })?;
    let loaded = crate::config_load::load_platform_config_from(
        record.config_path(),
        std::path::Path::new("/"),
    )?;
    if loaded.sha256 != record.config_sha256 {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "instance config changed while being reloaded",
        ));
    }
    if record.public_base_domain.is_some() && opts.daemon_gateway.is_none() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "public domain requires a shared gateway configuration",
        ));
    }
    let credentials = validate_registered_tokens(&refreshed, &opts.daemon_server)?;
    let credential = credentials
        .into_iter()
        .find(|credential| credential.instance_id == *id)
        .ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered identity changed",
            )
        })?;
    update_gateway_domain_records(&refreshed, gateway)?;
    api.refresh_stopped(record, credential)?;
    plan.records = refreshed;
    Ok(loaded)
}

#[allow(
    clippy::too_many_arguments,
    reason = "single daemon lifecycle boundary"
)]
fn attach_instance(
    plan: &mut DaemonPlan,
    config_path: &std::path::Path,
    autostart: bool,
    start: bool,
    gateway: Option<&gateway::GatewayOwner>,
    opts: &RunInner,
    instances: &mut RuntimeTasks,
    active: &mut HashMap<InstanceId, watch::Sender<bool>>,
    shutdowns: &mut Vec<watch::Sender<bool>>,
    daemon: Option<&daemon_control::DaemonApi>,
) -> Result<(), PlatformError> {
    let candidate =
        crate::config_load::load_platform_config_from(config_path, std::path::Path::new("/"))?;
    if candidate.config.public_gateway.is_some() && opts.daemon_gateway.is_none() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "public domain requires a shared gateway configuration",
        ));
    }
    let api = daemon.ok_or_else(|| {
        PlatformError::new(ErrorCode::RuntimeUnavailable, "daemon API is unavailable")
    })?;
    let (record, digest, credentials) = plan.registry.add_online(
        plan.scope,
        config_path,
        autostart,
        &plan.records,
        plan.manifest_digest.as_deref(),
    )?;
    plan.manifest_digest = digest;
    let id = record.instance_id()?;
    plan.records.push(record.clone());
    plan.records
        .sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    api.insert(&record, credentials)?;
    if let Err(error) = update_gateway_domains(plan, gateway) {
        api.mark(&id, "failed", Some(error.code()))?;
        return Err(error);
    }
    if !start {
        return Ok(());
    }
    let result = spawn_runtime(
        candidate,
        Some(id),
        opts,
        instances,
        active,
        shutdowns,
        daemon,
    );
    if let Err(error) = &result {
        api.mark(&id, "failed", Some(error.code()))?;
    }
    result
}

fn update_gateway_domains(
    plan: &DaemonPlan,
    gateway: Option<&gateway::GatewayOwner>,
) -> Result<(), PlatformError> {
    update_gateway_domain_records(&plan.records, gateway)
}

fn update_gateway_domain_records(
    records: &[InstanceRecord],
    gateway: Option<&gateway::GatewayOwner>,
) -> Result<(), PlatformError> {
    let Some(gateway) = gateway else {
        return Ok(());
    };
    let mut domains = records
        .iter()
        .filter_map(|record| record.public_base_domain.clone())
        .collect::<Vec<_>>();
    domains.sort_unstable();
    if gateway.control.domains()? != domains {
        gateway.update_domains(&domains)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "daemon_lifecycle_tests.rs"]
mod tests;
