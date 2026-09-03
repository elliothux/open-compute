//! Account and Script authority lookup for Worker v4 handlers.

use crate::http::HttpState;
use crate::workers_http::WorkerApiState;
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId};
use open_compute_storage::{WorkerRecord, WorkerRepository};

const MAX_WORKERS: u32 = 10_000;

pub(super) fn resolve_account(
    state: &HttpState,
    public_id: &str,
) -> Result<AccountId, crate::cloudflare_v4::V4Error> {
    state
        .cloudflare_v4_account()
        .ok_or(crate::cloudflare_v4::V4Error::Unavailable)?
        .resolve(public_id)
}

pub(super) fn worker_by_name(
    api: &WorkerApiState,
    account_id: AccountId,
    name: &str,
) -> Result<WorkerRecord, PlatformError> {
    WorkerRepository::new(api.storage.db())
        .list_workers(account_id)?
        .into_iter()
        .find(|worker| worker.name == name)
        .ok_or_else(|| PlatformError::new(ErrorCode::WorkerNotFound, "Worker was not found"))
}

pub(super) fn ensure_worker(
    api: &WorkerApiState,
    account_id: AccountId,
    name: &str,
    request_id: RequestId,
    now_ms: i64,
) -> Result<(WorkerRecord, bool), PlatformError> {
    match worker_by_name(api, account_id, name) {
        Ok(worker) => Ok((worker, false)),
        Err(error) if error.code() == ErrorCode::WorkerNotFound => {
            WorkerRepository::new(api.storage.db())
                .create_worker(account_id, name, request_id, now_ms, MAX_WORKERS)
                .map(|(worker, _)| (worker, true))
        }
        Err(error) => Err(error),
    }
}
