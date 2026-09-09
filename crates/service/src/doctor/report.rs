use super::*;

mod platform;
mod storage;

/// Run doctor against a loaded config.
pub async fn doctor_report(loaded: &LoadedConfig, mode: DoctorMode) -> DoctorReport {
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
