//! open-compute D1 migration lifecycle over the existing serialized D1 authority.

use super::*;
use axum::routing::get;
use open_compute_storage::{D1Migration, D1MigrationRecord};
use serde::{Deserialize, Serialize};

pub(super) fn router() -> Router<HttpState> {
    Router::new().route(
        "/accounts/{account_id}/open-compute/d1/databases/{database_id}/migrations",
        get(list).put(apply),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationInput {
    id: u32,
    name: String,
    sha256: String,
    sql: String,
}

#[derive(Serialize)]
struct MigrationOutput {
    id: u32,
    name: String,
    sha256: String,
    applied_at_ms: i64,
}

impl From<D1MigrationRecord> for MigrationOutput {
    fn from(value: D1MigrationRecord) -> Self {
        Self {
            id: value.id,
            name: value.name,
            sha256: value.sha256,
            applied_at_ms: value.applied_at_ms,
        }
    }
}

async fn list(
    State(state): State<HttpState>,
    Path((account, database)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let (account, database) = match resolve_resource(
        &state,
        &account,
        &database,
        V4ResourceKind::D1Database,
        BindingKind::D1Database,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    execute(&state, account, database, context, None).await
}

async fn apply(
    State(state): State<HttpState>,
    Path((account, database)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match request_context(&request) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = context.require(V4Permission::ProductWrite) {
        return error_response(error, context.request_id());
    }
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let (account, database) = match resolve_resource(
        &state,
        &account,
        &database,
        V4ResourceKind::D1Database,
        BindingKind::D1Database,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match crate::cloudflare_v4::storage::json::<Vec<MigrationInput>>(
        request,
        context.request_id(),
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let migrations = match body
        .into_iter()
        .map(|value| {
            let sha256: [u8; 32] = hex::decode(&value.sha256)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .filter(|_| {
                    value.sha256.len() == 64
                        && value
                            .sha256
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                .ok_or(V4Error::InvalidRequest)?;
            Ok(D1Migration {
                id: value.id,
                name: value.name,
                sha256,
                sql: value.sql,
            })
        })
        .collect::<Result<Vec<_>, V4Error>>()
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    execute(&state, account, database, context, Some(migrations)).await
}

async fn execute(
    state: &HttpState,
    account: InstanceId,
    database: ResourceId,
    context: V4RequestContext,
    migrations: Option<Vec<D1Migration>>,
) -> Response {
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let result = match migrations {
        Some(values) => {
            api.backend()
                .apply_migrations(account, database, values, open_compute_core::wall_time_ms())
                .await
        }
        None => api.backend().migrations(account, database).await,
    };
    match result {
        Ok(records) => success_response(
            context,
            records
                .into_iter()
                .map(MigrationOutput::from)
                .collect::<Vec<_>>(),
        ),
        Err(error) if error.code() == ErrorCode::D1MigrationDrift => {
            error_response(V4Error::Conflict, context.request_id())
        }
        Err(error) => platform_error(&error, context),
    }
}
