//! Explicit, fail-closed removal of registered local instance state.

use crate::config_load::{lexical_absolute, load_platform_config_from};
use crate::instance_ops::{INSTANCE_STOP_TIMEOUT, wait_until_instance_quiescent};
use crate::instance_registry::{
    InstanceRecord, InstanceRegistry, RegisteredObjectAuthority, current_binary_path,
};
use crate::service_manager::ServiceManager;
use open_compute_artifacts::ObjectBackend;
use open_compute_core::{
    ErrorCode, InstanceSelector, ObjectStorageConfig, ObjectStorageKind, PlatformError,
};
use open_compute_storage::inspect_control_db;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
struct PurgePlan {
    record: InstanceRecord,
    config_path: PathBuf,
    config_sha256: String,
    data_dir: PathBuf,
    external_local_root: Option<PathBuf>,
    retained_local_root: Option<PathBuf>,
    external_authority: Option<String>,
}

/// Select and purge exactly one registered instance.
#[allow(
    clippy::too_many_arguments,
    reason = "CLI boundary mirrors explicit purge inputs"
)]
pub(crate) fn run_selected_purge(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    yes: bool,
    dry_run: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let record = select_record(config, instance, startup_cwd, registry)?;
    let current = current_binary_path()?;
    if record.binary_path() != current.as_path() {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "selected instance belongs to a different ocd installation",
        ));
    }
    purge_records(
        &[record],
        registry,
        manager,
        runtime_root,
        yes,
        dry_run,
        out,
    )
}

/// Return only registrations owned by `binary_path`.
pub(crate) fn owned_records(
    registry: &InstanceRegistry,
    binary_path: &Path,
) -> Result<Vec<InstanceRecord>, PlatformError> {
    Ok(registry
        .list()?
        .into_iter()
        .filter(|record| record.binary_path() == binary_path)
        .collect())
}

/// Stop and unregister owned instances while retaining their local state.
pub(crate) fn unregister_preserving_data(
    records: &[InstanceRecord],
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    dry_run: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let plans = build_plans(records, registry, false)?;
    for plan in &plans {
        write_plan(plan, "UNINSTALL_RETAIN", out)?;
    }
    if dry_run {
        return Ok(());
    }
    for plan in &plans {
        stop_and_unregister(plan, registry, manager, runtime_root, out)?;
    }
    Ok(())
}

/// Purge all supplied records after validating the complete deletion set.
pub(crate) fn purge_records(
    records: &[InstanceRecord],
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    yes: bool,
    dry_run: bool,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    let plans = build_plans(records, registry, true)?;
    for plan in &plans {
        write_plan(plan, "PURGE_PLAN", out)?;
    }
    if dry_run {
        writeln!(out, "PURGE_DRY_RUN_OK instances={}", plans.len()).map_err(|_| io_failed())?;
        return Ok(());
    }
    let stdin = std::io::stdin();
    let stderr = std::io::stderr();
    confirm(
        &plans,
        yes,
        stdin.is_terminal(),
        &mut stdin.lock(),
        &mut stderr.lock(),
    )?;
    for plan in &plans {
        if let Err(error) = stop_and_uninstall_service(plan, manager, runtime_root) {
            writeln!(
                out,
                "PURGE_INSTANCE_FAILED {} local_state=retained",
                plan.record.instance_id
            )
            .map_err(|_| io_failed())?;
            return Err(error);
        }
        writeln!(
            out,
            "PURGE_SERVICE_UNREGISTERED {}",
            plan.record.instance_id
        )
        .map_err(|_| io_failed())?;
    }
    for plan in &plans {
        if let Err(error) = verify_config_unchanged(plan) {
            write_remaining(&plans, out)?;
            return Err(error);
        }
    }
    for plan in &plans {
        if let Err(error) = delete_state_roots(plan, out) {
            write_remaining(&plans, out)?;
            return Err(error);
        }
    }
    for plan in &plans {
        let selector = InstanceSelector::from(plan.record.instance_id()?);
        if let Err(error) = registry.remove(&selector) {
            write_remaining(&plans, out)?;
            return Err(error);
        }
        writeln!(out, "INSTANCE_UNREGISTERED {}", plan.record.instance_id)
            .map_err(|_| io_failed())?;
        if let Err(error) = delete_config(plan, out) {
            write_remaining(&plans, out)?;
            return Err(error);
        }
    }
    writeln!(out, "PURGE_OK instances={}", plans.len()).map_err(|_| io_failed())?;
    Ok(())
}

fn select_record(
    config: Option<&Path>,
    instance: Option<&InstanceSelector>,
    startup_cwd: &Path,
    registry: &InstanceRegistry,
) -> Result<InstanceRecord, PlatformError> {
    match (config, instance) {
        (None, None) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "purge requires exactly one `--instance <id>` or `--config <exact-path>` selector",
        )),
        (Some(_), Some(_)) => Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "--instance and --config are mutually exclusive",
        )),
        (None, Some(selector)) => registry.get(selector),
        (Some(config), None) => {
            let exact = lexical_absolute(startup_cwd, config)?;
            let loaded = load_platform_config_from(&exact, startup_cwd)?;
            registry
                .list()?
                .into_iter()
                .find(|record| record.config_path() == loaded.path)
                .ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::InstanceNotFound,
                        "the exact purge configuration is not registered",
                    )
                })
        }
    }
}

fn build_plans(
    records: &[InstanceRecord],
    registry: &InstanceRegistry,
    destructive: bool,
) -> Result<Vec<PurgePlan>, PlatformError> {
    let all_records = registry.list()?;
    let mut plans = Vec::with_capacity(records.len());
    for record in records {
        let config_path = record.config_path().to_owned();
        let mut loaded_config = None;
        let config_sha256 = if destructive {
            let loaded = load_platform_config_from(&config_path, Path::new("/"))?;
            let actual = loaded.sha256.clone();
            if actual != record.config_sha256
                || loaded.config.data.path != Path::new(&record.data_path)
            {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "configuration changed after instance registration; re-register before purge",
                ));
            }
            loaded_config = Some(loaded.config);
            actual
        } else {
            record.config_sha256.clone()
        };
        let data_dir = PathBuf::from(&record.data_path);
        let (external_local_root, retained_local_root, external_authority) =
            match &record.object_authority {
                RegisteredObjectAuthority::Local { path } => {
                    let path = PathBuf::from(path);
                    let nested = data_dir.join("objects");
                    if path == nested {
                        (None, None, None)
                    } else if destructive
                        && loaded_config
                            .as_ref()
                            .is_some_and(local_authority_is_uniquely_owned)
                    {
                        (Some(path), None, None)
                    } else {
                        (None, Some(path), None)
                    }
                }
                RegisteredObjectAuthority::S3 { endpoint, bucket } => (
                    None,
                    None,
                    Some(format!("s3 endpoint={endpoint} bucket={bucket}")),
                ),
            };
        let plan = PurgePlan {
            record: record.clone(),
            config_path,
            config_sha256,
            data_dir,
            external_local_root,
            retained_local_root,
            external_authority,
        };
        validate_printable_plan(&plan)?;
        if destructive {
            validate_plan_paths(&plan)?;
        }
        plans.push(plan);
    }
    if destructive {
        validate_no_overlaps(&plans, &all_records)?;
        for plan in &plans {
            validate_delete_tree(&plan.data_dir)?;
            if let Some(path) = &plan.external_local_root {
                validate_delete_tree(path)?;
            }
        }
    }
    Ok(plans)
}

fn validate_printable_plan(plan: &PurgePlan) -> Result<(), PlatformError> {
    let unsafe_path = [&plan.config_path, &plan.data_dir]
        .into_iter()
        .chain(plan.external_local_root.iter())
        .chain(plan.retained_local_root.iter())
        .any(|path| path.to_string_lossy().chars().any(char::is_control));
    let unsafe_authority = plan
        .external_authority
        .as_deref()
        .is_some_and(|value| value.chars().any(char::is_control));
    if unsafe_path || unsafe_authority {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "lifecycle plan contains control characters",
        ));
    }
    Ok(())
}

fn local_authority_is_uniquely_owned(config: &open_compute_core::PlatformConfig) -> bool {
    let ObjectStorageConfig::Local(local) = &config.object_storage else {
        return false;
    };
    if !local.path.exists() {
        return false;
    }
    let Ok((_, identity)) = inspect_control_db(
        &config.data.path.join("control.sqlite"),
        config.data.sqlite_busy_timeout_ms,
    ) else {
        return false;
    };
    let Ok((platform_id, authority_sha256, _)) = ObjectBackend::inspect_local_authority(local)
    else {
        return false;
    };
    identity.platform_id == platform_id
        && identity.object_backend_kind == Some(ObjectStorageKind::Local)
        && identity.object_authority_sha256 == Some(authority_sha256)
}

fn validate_plan_paths(plan: &PurgePlan) -> Result<(), PlatformError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for path in [&plan.config_path, &plan.data_dir]
        .into_iter()
        .chain(plan.external_local_root.iter())
    {
        if !path.is_absolute()
            || path == Path::new("/")
            || home.as_deref() == Some(path.as_path())
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "purge target is not a safe exact absolute path",
            ));
        }
        if let Ok(meta) = fs::symlink_metadata(path)
            && meta.file_type().is_symlink()
        {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "purge refuses a symlink target",
            ));
        }
        validate_no_symlink_ancestors(path)?;
    }
    if let Ok(meta) = fs::symlink_metadata(&plan.config_path)
        && (!meta.is_file() || meta.nlink() != 1)
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "purge config must be a uniquely owned regular file",
        ));
    }
    let mut roots = vec![&plan.data_dir];
    if let Some(path) = &plan.external_local_root {
        roots.push(path);
    }
    if roots.iter().any(|root| plan.config_path.starts_with(root)) {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "purge config and data roots must not overlap",
        ));
    }
    Ok(())
}

fn validate_no_overlaps(
    plans: &[PurgePlan],
    all_records: &[InstanceRecord],
) -> Result<(), PlatformError> {
    let selected: std::collections::HashSet<&str> = plans
        .iter()
        .map(|plan| plan.record.instance_id.as_str())
        .collect();
    let mut protected = Vec::new();
    for record in all_records {
        if selected.contains(record.instance_id.as_str()) {
            continue;
        }
        protected.push(PathBuf::from(&record.data_path));
        if let RegisteredObjectAuthority::Local { path } = &record.object_authority {
            protected.push(PathBuf::from(path));
        }
        protected.push(record.config_path().to_owned());
    }
    let deletion_roots = plans
        .iter()
        .flat_map(|plan| std::iter::once(&plan.data_dir).chain(plan.external_local_root.iter()))
        .collect::<Vec<_>>();
    for (index, root) in deletion_roots.iter().enumerate() {
        if deletion_roots
            .iter()
            .skip(index + 1)
            .any(|other| overlaps(root, other))
            || protected.iter().any(|other| overlaps(root, other))
        {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "purge refuses overlapping instance roots",
            ));
        }
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_delete_tree(root: &Path) -> Result<(), PlatformError> {
    validate_no_symlink_ancestors(root)?;
    let meta = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to inspect purge root",
            ));
        }
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "purge root must be a real directory",
        ));
    }
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|_| PlatformError::new(ErrorCode::PathInvalid, "failed to read purge tree"))?
        {
            let entry = entry.map_err(|_| {
                PlatformError::new(ErrorCode::PathInvalid, "failed to read purge tree entry")
            })?;
            let meta = fs::symlink_metadata(entry.path()).map_err(|_| {
                PlatformError::new(ErrorCode::PathInvalid, "failed to inspect purge tree entry")
            })?;
            if meta.file_type().is_symlink() {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "purge tree contains a symlink",
                ));
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else if !meta.is_file() || meta.nlink() != 1 {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "purge tree contains an ambiguous filesystem entry",
                ));
            }
        }
    }
    Ok(())
}

fn validate_no_symlink_ancestors(path: &Path) -> Result<(), PlatformError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "purge target has a symlink ancestor",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "failed to inspect purge target ancestry",
                ));
            }
        }
    }
    Ok(())
}

fn write_plan(plan: &PurgePlan, prefix: &str, out: &mut impl Write) -> Result<(), PlatformError> {
    writeln!(
        out,
        "{prefix} instance={} config={} data_dir={}",
        plan.record.instance_id,
        plan.config_path.display(),
        plan.data_dir.display()
    )
    .map_err(|_| io_failed())?;
    if let Some(path) = &plan.external_local_root {
        writeln!(out, "{prefix}_LOCAL_OBJECTS {}", path.display()).map_err(|_| io_failed())?;
    }
    if let Some(path) = &plan.retained_local_root {
        writeln!(
            out,
            "{prefix}_EXTERNAL_RETAINED local path={} ownership=unproven; delete manually if intended",
            path.display()
        )
        .map_err(|_| io_failed())?;
    }
    if let Some(authority) = &plan.external_authority {
        writeln!(
            out,
            "{prefix}_EXTERNAL_RETAINED {authority}; delete external objects manually if intended"
        )
        .map_err(|_| io_failed())?;
    }
    Ok(())
}

fn confirm(
    plans: &[PurgePlan],
    yes: bool,
    terminal: bool,
    input: &mut impl BufRead,
    error: &mut impl Write,
) -> Result<(), PlatformError> {
    if yes || plans.is_empty() {
        return Ok(());
    }
    if !terminal {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "non-interactive purge requires `--yes`",
        ));
    }
    let confirmation = plans
        .iter()
        .map(|plan| {
            let mut item = format!(
                "instance={} config={} data={}",
                plan.record.instance_id,
                plan.config_path.display(),
                plan.data_dir.display()
            );
            if let Some(path) = &plan.external_local_root {
                item.push_str(&format!(" local_objects={}", path.display()));
            }
            if let Some(path) = &plan.retained_local_root {
                item.push_str(&format!(" retained_local_objects={}", path.display()));
            }
            if let Some(authority) = &plan.external_authority {
                item.push_str(&format!(" retained={authority}"));
            }
            item
        })
        .collect::<Vec<_>>()
        .join("; ");
    let confirmation = format!("purge {confirmation}");
    writeln!(
        error,
        "Type `{confirmation}` to irreversibly delete the listed local paths:"
    )
    .map_err(|_| io_failed())?;
    let mut line = String::new();
    input.read_line(&mut line).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to read purge confirmation",
        )
    })?;
    if line.trim() != confirmation {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "purge confirmation did not match the exact instance set",
        ));
    }
    Ok(())
}

fn stop_and_unregister(
    plan: &PurgePlan,
    registry: &InstanceRegistry,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
    out: &mut impl Write,
) -> Result<(), PlatformError> {
    stop_and_uninstall_service(plan, manager, runtime_root)?;
    let selector = InstanceSelector::from(plan.record.instance_id()?);
    registry.remove(&selector)?;
    writeln!(out, "INSTANCE_UNREGISTERED {}", plan.record.instance_id).map_err(|_| io_failed())?;
    Ok(())
}

fn stop_and_uninstall_service(
    plan: &PurgePlan,
    manager: &dyn ServiceManager,
    runtime_root: Option<&Path>,
) -> Result<(), PlatformError> {
    if manager.is_active(&plan.record)? {
        manager.stop(&plan.record)?;
    }
    wait_until_instance_quiescent(
        &plan.record,
        &plan.data_dir.join("platform.lock"),
        runtime_root,
        manager,
        INSTANCE_STOP_TIMEOUT,
    )?;
    manager.uninstall(&plan.record)?;
    Ok(())
}

fn verify_config_unchanged(plan: &PurgePlan) -> Result<(), PlatformError> {
    let bytes = fs::read(&plan.config_path).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "purge config disappeared before deletion",
        )
    })?;
    if hex::encode(Sha256::digest(bytes)) != plan.config_sha256 {
        return Err(PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "purge config changed after planning; local state was retained",
        ));
    }
    Ok(())
}

fn delete_state_roots(plan: &PurgePlan, out: &mut impl Write) -> Result<(), PlatformError> {
    if let Some(path) = &plan.external_local_root {
        validate_delete_tree(path)?;
        remove_tree(path)?;
        writeln!(out, "PURGE_REMOVED {}", path.display()).map_err(|_| io_failed())?;
    }
    validate_delete_tree(&plan.data_dir)?;
    remove_tree(&plan.data_dir)?;
    writeln!(out, "PURGE_REMOVED {}", plan.data_dir.display()).map_err(|_| io_failed())?;
    Ok(())
}

fn delete_config(plan: &PurgePlan, out: &mut impl Write) -> Result<(), PlatformError> {
    validate_no_symlink_ancestors(&plan.config_path)?;
    match fs::remove_file(&plan.config_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to remove purge config",
            ));
        }
    }
    writeln!(out, "PURGE_REMOVED {}", plan.config_path.display()).map_err(|_| io_failed())?;
    Ok(())
}

fn remove_tree(path: &Path) -> Result<(), PlatformError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "failed to remove purge tree; previously reported removals are complete and unreported paths remain",
        )),
    }
}

fn write_remaining(plans: &[PurgePlan], out: &mut impl Write) -> Result<(), PlatformError> {
    for plan in plans {
        for path in [&plan.config_path, &plan.data_dir]
            .into_iter()
            .chain(plan.external_local_root.iter())
        {
            if fs::symlink_metadata(path).is_ok() {
                writeln!(out, "PURGE_REMAINS {}", path.display()).map_err(|_| io_failed())?;
            }
        }
        if let Some(authority) = &plan.external_authority {
            writeln!(out, "PURGE_REMAINS {authority}").map_err(|_| io_failed())?;
        }
        if let Some(path) = &plan.retained_local_root {
            writeln!(out, "PURGE_REMAINS local path={}", path.display())
                .map_err(|_| io_failed())?;
        }
    }
    Ok(())
}

fn io_failed() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "failed to write lifecycle output")
}

#[cfg(test)]
#[path = "instance_purge_tests.rs"]
mod tests;
