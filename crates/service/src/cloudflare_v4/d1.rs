//! Official Cloudflare v4 D1 catalog and SQL adapter.

use super::storage::{
    account, context, iso_timestamp, json, now_ms, require_no_query, resolve_resource_id,
    strict_query,
};
use super::{
    V4Error, V4Permission, V4ResourceKind, error_response, paginated_response, success_response,
};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::BindingKind;
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, D1DatabaseRecord, D1DatabaseRepository, D1Statement,
    D1StatementResult, D1Value,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const KIND: V4ResourceKind = V4ResourceKind::D1Database;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/d1/database",
            post(create_database).get(list_databases),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}",
            get(get_database)
                .put(update_database)
                .patch(update_database)
                .delete(delete_database),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/query",
            post(query_database),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/raw",
            post(raw_database),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateDatabase {
    name: String,
    jurisdiction: Option<String>,
    primary_location_hint: Option<String>,
    read_replication: Option<ReadReplication>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadReplication {
    mode: String,
}

#[derive(Serialize)]
struct Database {
    uuid: String,
    name: String,
    created_at: String,
    version: &'static str,
    jurisdiction: Option<String>,
    read_replication: Replication,
}

#[derive(Serialize)]
struct Replication {
    mode: &'static str,
}

impl Database {
    fn from_record(
        authority: &super::accounts::AccountAuthority,
        record: &D1DatabaseRecord,
    ) -> Result<Self, V4Error> {
        Ok(Self {
            uuid: authority.public_resource_id(KIND, record.resource.id),
            name: record.resource.name.clone(),
            created_at: iso_timestamp(record.resource.created_at_ms)?,
            version: "production",
            jurisdiction: None,
            read_replication: Replication { mode: "disabled" },
        })
    }
}

async fn create_database(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json::<CreateDatabase>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_database_name(&body.name) {
        return error_response(V4Error::InvalidField("/name"), context.request_id());
    }
    if body
        .jurisdiction
        .as_deref()
        .is_some_and(|value| !matches!(value, "eu" | "fedramp" | "us"))
    {
        return error_response(V4Error::InvalidField("/jurisdiction"), context.request_id());
    }
    if body
        .primary_location_hint
        .as_deref()
        .is_some_and(|value| !matches!(value, "wnam" | "enam" | "weur" | "eeur" | "apac" | "oc"))
    {
        return error_response(
            V4Error::InvalidField("/primary_location_hint"),
            context.request_id(),
        );
    }
    if body
        .read_replication
        .as_ref()
        .is_some_and(|replication| !matches!(replication.mode.as_str(), "auto" | "disabled"))
    {
        return error_response(
            V4Error::InvalidField("/read_replication/mode"),
            context.request_id(),
        );
    }
    if body.jurisdiction.is_some()
        || body.primary_location_hint.is_some()
        || body
            .read_replication
            .as_ref()
            .is_some_and(|replication| replication.mode == "auto")
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let Some(api) = state.d1_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let driver = D1ResourceDriver::new(api.storage(), api.config().database_quota_bytes);
        let outcome = ResourceController::new(api.storage(), api.pins().clone(), driver)
            .create(&CreateResourceRequest {
                account_id,
                kind: BindingKind::D1Database,
                name: body.name,
                idempotency_key: request_id.to_string(),
                driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
                request_id,
                now_ms: now_ms()?,
            })
            .map_err(|error| V4Error::from(&error))?;
        let resource_id = match outcome {
            CreateResourceOutcome::Applied(value) => value.resource_id,
            CreateResourceOutcome::Replay(_) => return Err(V4Error::Conflict),
        };
        D1DatabaseRepository::new(api.storage().db())
            .get(account_id, resource_id)
            .map_err(|error| V4Error::from(&error))
    })
    .await;
    match result {
        Ok(Ok(record)) => match state.cloudflare_v4_account() {
            Some(authority) => match Database::from_record(authority, &record) {
                Ok(database) => success_response(context, database),
                Err(error) => error_response(error, request_id),
            },
            None => error_response(V4Error::Unavailable, request_id),
        },
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

struct ListQuery {
    page: usize,
    per_page: usize,
    name: Option<String>,
}

const fn one() -> usize {
    1
}

const fn twenty() -> usize {
    20
}

async fn list_databases(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match list_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query.page == 0 || query.per_page == 0 || query.per_page > 1000 {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let records = match D1DatabaseRepository::new(api.storage().db()).list(account_id) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let filtered: Vec<_> = records
        .iter()
        .filter(|record| {
            query
                .name
                .as_deref()
                .is_none_or(|name| record.resource.name.contains(name))
        })
        .collect();
    let total = filtered.len();
    let start = query.page.saturating_sub(1).saturating_mul(query.per_page);
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let databases: Result<Vec<_>, _> = filtered
        .into_iter()
        .skip(start)
        .take(query.per_page)
        .map(|record| Database::from_record(authority, record))
        .collect();
    let databases = match databases {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let count = databases.len();
    paginated_response(
        context,
        databases,
        super::V4ResultInfo {
            page: query.page,
            per_page: query.per_page,
            count,
            total_count: total,
            total_pages: total.div_ceil(query.per_page),
        },
    )
}

async fn get_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let fields = match database_fields(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (_, record) = match database_with_context(&state, context, &account_id, &database_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match Database::from_record(authority, &record) {
        Ok(database) => match filtered_database(database, fields.as_deref()) {
            Ok(database) => success_response(context, database),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => error_response(error, context.request_id()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateDatabase {
    read_replication: Option<ReadReplication>,
}

async fn update_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let require_replication = request.method() == axum::http::Method::PUT;
    let (context, _, record) = match database(&state, &request, &account_id, &database_id, true) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<UpdateDatabase>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if require_replication && body.read_replication.is_none() {
        return error_response(
            V4Error::InvalidField("/read_replication"),
            context.request_id(),
        );
    }
    if body
        .read_replication
        .as_ref()
        .is_some_and(|replication| !matches!(replication.mode.as_str(), "auto" | "disabled"))
    {
        return error_response(
            V4Error::InvalidField("/read_replication/mode"),
            context.request_id(),
        );
    }
    if body
        .read_replication
        .as_ref()
        .is_some_and(|replication| replication.mode == "auto")
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match Database::from_record(authority, &record) {
        Ok(database) => success_response(context, database),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn delete_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match database(&state, &request, &account_id, &database_id, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.d1_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let driver = D1ResourceDriver::new(api.storage(), api.config().database_quota_bytes);
    match ResourceController::new(api.storage(), api.pins().clone(), driver)
        .delete(
            account_id,
            record.resource.id,
            request_id,
            now,
            api.delete_drain_timeout(),
        )
        .await
    {
        Ok(()) => success_response(context, ()),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum QueryBody {
    Single(Statement),
    Batch(BatchStatements),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchStatements {
    batch: Vec<Statement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Statement {
    sql: String,
    #[serde(default)]
    params: Vec<String>,
}

impl QueryBody {
    fn statements(self) -> Vec<D1Statement> {
        let statements = match self {
            Self::Single(statement) => vec![statement],
            Self::Batch(BatchStatements { batch }) => batch,
        };
        statements
            .into_iter()
            .map(|statement| D1Statement {
                sql: statement.sql,
                params: statement.params.into_iter().map(D1Value::Text).collect(),
            })
            .collect()
    }
}

async fn query_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    run_query(state, account_id, database_id, request, false).await
}

async fn raw_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    run_query(state, account_id, database_id, request, true).await
}

async fn run_query(
    state: HttpState,
    account_id: String,
    database_id: String,
    request: Request,
    raw: bool,
) -> Response {
    let (context, account_id, record) =
        match database(&state, &request, &account_id, &database_id, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<QueryBody>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let results = match api
        .backend()
        .cloudflare_v4_query(account_id, record.resource.id, body.statements())
        .await
    {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    if raw {
        match results
            .into_iter()
            .map(raw_result)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(results) => success_response(context, results),
            Err(error) => error_response(error, context.request_id()),
        }
    } else {
        match results
            .into_iter()
            .map(object_result)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(results) => success_response(context, results),
            Err(error) => error_response(error, context.request_id()),
        }
    }
}

#[derive(Serialize)]
struct ObjectResult {
    success: bool,
    results: Vec<Map<String, Value>>,
    meta: open_compute_storage::D1Meta,
}

#[derive(Serialize)]
struct RawResult {
    success: bool,
    results: RawRows,
    meta: open_compute_storage::D1Meta,
}

#[derive(Serialize)]
struct RawRows {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

fn object_result(result: D1StatementResult) -> Result<ObjectResult, V4Error> {
    let rows = result
        .rows
        .into_iter()
        .map(|row| {
            result
                .columns
                .iter()
                .cloned()
                .zip(row.into_iter().map(value))
                .map(|(column, value)| value.map(|value| (column, value)))
                .collect()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ObjectResult {
        success: true,
        results: rows,
        meta: result.meta,
    })
}

fn raw_result(result: D1StatementResult) -> Result<RawResult, V4Error> {
    Ok(RawResult {
        success: true,
        results: RawRows {
            columns: result.columns,
            rows: result
                .rows
                .into_iter()
                .map(|row| row.into_iter().map(value).collect())
                .collect::<Result<Vec<_>, _>>()?,
        },
        meta: result.meta,
    })
}

fn value(value: D1Value) -> Result<Value, V4Error> {
    Ok(match value {
        D1Value::Null => Value::Null,
        D1Value::Integer(value) => Value::from(value),
        D1Value::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or(V4Error::Internal)?,
        D1Value::Text(value) => Value::String(value),
        D1Value::Blob(value) => Value::Array(value.into_iter().map(Value::from).collect()),
    })
}

fn valid_database_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'_' | b'-'))
        })
}

fn database_fields(request: &Request) -> Result<Option<Vec<String>>, V4Error> {
    let mut values = strict_query(request)?;
    let fields = values.remove("fields").map(|value| {
        value
            .split(',')
            .map(|field| {
                if matches!(
                    field,
                    "uuid"
                        | "name"
                        | "created_at"
                        | "version"
                        | "jurisdiction"
                        | "num_tables"
                        | "file_size"
                        | "running_in_region"
                        | "read_replication"
                ) {
                    Ok(field.to_owned())
                } else {
                    Err(V4Error::InvalidRequest)
                }
            })
            .collect::<Result<Vec<_>, _>>()
    });
    if !values.is_empty() {
        return Err(V4Error::InvalidRequest);
    }
    let fields = fields.transpose()?;
    if fields.as_ref().is_some_and(Vec::is_empty) {
        return Err(V4Error::InvalidRequest);
    }
    Ok(fields)
}

fn filtered_database(database: Database, fields: Option<&[String]>) -> Result<Value, V4Error> {
    let mut value = serde_json::to_value(database).map_err(|_| V4Error::Internal)?;
    if let Some(fields) = fields {
        let object = value.as_object_mut().ok_or(V4Error::Internal)?;
        object.retain(|key, _| fields.iter().any(|field| field == key));
    }
    Ok(value)
}

fn list_query(request: &Request) -> Result<ListQuery, V4Error> {
    let mut values = strict_query(request)?;
    let parse = |value: Option<String>, default| {
        value
            .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
            .transpose()
            .map(|value| value.unwrap_or(default))
    };
    let page = parse(values.remove("page"), one())?;
    let per_page = parse(values.remove("per_page"), twenty())?;
    let name = values.remove("name");
    if !values.is_empty() {
        return Err(V4Error::InvalidRequest);
    }
    Ok(ListQuery {
        page,
        per_page,
        name,
    })
}

pub(super) fn database(
    state: &HttpState,
    request: &Request,
    account_id: &str,
    database_id: &str,
    write: bool,
) -> Result<
    (
        super::V4RequestContext,
        open_compute_core::AccountId,
        D1DatabaseRecord,
    ),
    Response,
> {
    let context = context(
        request,
        if write {
            V4Permission::ProductWrite
        } else {
            V4Permission::Read
        },
    )?;
    let (account_id, record) = database_with_context(state, context, account_id, database_id)?;
    Ok((context, account_id, record))
}

fn database_with_context(
    state: &HttpState,
    context: super::V4RequestContext,
    account_id: &str,
    database_id: &str,
) -> Result<(open_compute_core::AccountId, D1DatabaseRecord), Response> {
    let account_id =
        account(state, account_id).map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .d1_api()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let records = D1DatabaseRepository::new(api.storage().db())
        .list(account_id)
        .map_err(|error| error_response(V4Error::from(&error), context.request_id()))?;
    let authority = state
        .cloudflare_v4_account()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let resource_id = resolve_resource_id(
        authority,
        KIND,
        database_id,
        &records,
        |record: &D1DatabaseRecord| record.resource.id,
    )
    .map_err(|error| error_response(error, context.request_id()))?;
    let record = records
        .into_iter()
        .find(|record| record.resource.id == resource_id)
        .ok_or_else(|| error_response(V4Error::NotFound, context.request_id()))?;
    Ok((account_id, record))
}

#[cfg(test)]
mod tests {
    use super::{QueryBody, valid_database_name, value};
    use open_compute_storage::D1Value;

    #[test]
    fn database_names_and_non_finite_values_fail_closed() {
        assert!(valid_database_name("db-name_1"));
        assert!(!valid_database_name("-db"));
        assert!(!valid_database_name("db.name"));
        assert!(value(D1Value::Real(f64::NAN)).is_err());
        assert!(value(D1Value::Real(f64::INFINITY)).is_err());
    }

    #[test]
    fn batch_shape_rejects_unknown_fields_and_matches_frozen_empty_array_contract() {
        assert!(
            serde_json::from_value::<QueryBody>(serde_json::json!({
                "batch": [],
                "unknown": true
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<QueryBody>(serde_json::json!({ "batch": [] })).is_ok());
    }
}
