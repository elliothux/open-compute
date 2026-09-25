//! Instance and Script authority lookup for Worker v4 handlers.

use crate::http::HttpState;
use crate::workers_http::WorkerApiState;
use open_compute_core::{ErrorCode, InstanceId, PlatformError, RequestId};
use open_compute_storage::{WorkerRecord, WorkerRepository};

const MAX_WORKERS: u32 = 10_000;

pub(super) fn resolve_instance(
    state: &HttpState,
    public_id: &str,
) -> Result<InstanceId, crate::cloudflare_v4::V4Error> {
    state
        .v4_instance_context()
        .ok_or(crate::cloudflare_v4::V4Error::Unavailable)?
        .resolve(public_id)
}

pub(super) fn worker_by_name(
    api: &WorkerApiState,
    instance_id: InstanceId,
    name: &str,
) -> Result<WorkerRecord, PlatformError> {
    WorkerRepository::new(api.storage.db())
        .list_workers(instance_id)?
        .into_iter()
        .find(|worker| worker.name == name)
        .ok_or_else(|| PlatformError::new(ErrorCode::WorkerNotFound, "Worker was not found"))
}

pub(super) fn ensure_worker(
    api: &WorkerApiState,
    instance_id: InstanceId,
    name: &str,
    request_id: RequestId,
    now_ms: i64,
) -> Result<(WorkerRecord, bool), PlatformError> {
    if api.local_extension_exists(name) {
        return Err(PlatformError::new(
            ErrorCode::WorkerNameConflict,
            "Worker name conflicts with a configured local extension",
        ));
    }
    match worker_by_name(api, instance_id, name) {
        Ok(worker) => Ok((worker, false)),
        Err(error) if error.code() == ErrorCode::WorkerNotFound => {
            WorkerRepository::new(api.storage.db())
                .create_worker(instance_id, name, request_id, now_ms, MAX_WORKERS)
                .map(|(worker, _)| (worker, true))
        }
        Err(error) => Err(error),
    }
}
