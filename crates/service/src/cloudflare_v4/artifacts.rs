//! Official Cloudflare Artifacts namespace and repository management routes.

mod content;
mod pagination;

use self::pagination::{cursor_page, offset_page};
use super::storage::{account, context, iso_timestamp, json, now_ms, require_no_query};
use super::{V4Error, V4OfficialError, V4Permission, error_response, success_response};
use crate::artifact_api::{
    ArtifactApiState, CreateRepositoryRequest, ForkRepositoryRequest, ImportRepositoryRequest,
    IssuedArtifactToken,
};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, post};
use open_compute_core::{ArtifactTokenId, ErrorCode, PlatformError, RequestId};
use open_compute_storage::{
    ArtifactNamespaceRecord, ArtifactRepositoryRecord, ArtifactTokenRecord, ArtifactTokenScope,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/artifacts/namespaces",
            post(create_namespace).get(list_namespaces),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}",
            get(get_namespace),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos",
            post(create_repository).get(list_repositories),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}",
            get(get_repository).delete(delete_repository),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/fork",
            post(fork_repository),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/import",
            post(import_repository),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/tokens",
            post(issue_token),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/tokens",
            get(list_tokens),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/tokens/{token}",
            delete(revoke_token),
        )
        .merge(content::router())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNamespaceBody {
    namespace: String,
    jurisdiction: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRepositoryBody {
    name: String,
    description: Option<String>,
    default_branch: Option<String>,
    read_only: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkBody {
    name: String,
    description: Option<String>,
    read_only: Option<bool>,
    default_branch_only: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    url: String,
    branch: Option<String>,
    depth: Option<u32>,
    read_only: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueTokenBody {
    repo: String,
    scope: Option<String>,
    ttl: Option<u32>,
}

#[derive(Serialize)]
struct NamespaceDto {
    namespace: String,
    repo_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    jurisdiction: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct RepositoryDto {
    id: String,
    name: String,
    description: Option<String>,
    default_branch: String,
    remote: String,
    read_only: bool,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_push_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(Serialize)]
struct CreatedRepositoryDto {
    id: String,
    name: String,
    description: Option<String>,
    default_branch: String,
    remote: String,
    token: String,
}

#[derive(Serialize)]
struct ForkedRepositoryDto {
    #[serde(flatten)]
    repository: CreatedRepositoryDto,
    objects: usize,
}

#[derive(Serialize)]
struct TokenDto {
    id: String,
    scope: ArtifactTokenScope,
    expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plaintext: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    page: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoriesQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    page: Option<usize>,
    search: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokensQuery {
    state: Option<String>,
    per_page: Option<usize>,
    page: Option<usize>,
}

async fn create_namespace(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(body) = json::<CreateNamespaceBody>(request, context.request_id()).await else {
        return invalid_response(context.request_id());
    };
    if body.jurisdiction.is_some() {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    match api
        .create_namespace(account, &body.namespace, now_ms())
        .and_then(|record| namespace_dto(api, account, &record))
    {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn list_namespaces(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match prepare(&state, &account_id, &request, V4Permission::Read, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let Ok(query) = parse_query::<CursorQuery>(&request) else {
        return invalid_response(context.request_id());
    };
    match api.list_namespaces(account).and_then(|records| {
        records
            .iter()
            .map(|record| namespace_dto(api, account, record))
            .collect::<Result<Vec<_>, _>>()
    }) {
        Ok(result) => cursor_page(
            context,
            result,
            query.limit,
            query.cursor.as_deref(),
            query.page,
        ),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn get_namespace(
    State(state): State<HttpState>,
    Path((account_id, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match prepare(&state, &account_id, &request, V4Permission::Read, false) {
            Ok(value) => value,
            Err(response) => return response,
        };
    match api
        .namespace(account, &namespace)
        .and_then(|record| namespace_dto(api, account, &record))
    {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn create_repository(
    State(state): State<HttpState>,
    Path((account_id, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(body) = json::<CreateRepositoryBody>(request, context.request_id()).await else {
        return invalid_response(context.request_id());
    };
    let result = api
        .create_repository(
            account,
            &namespace,
            CreateRepositoryRequest {
                name: &body.name,
                description: body.description.as_deref().unwrap_or(""),
                default_branch: body.default_branch.as_deref().unwrap_or("main"),
                read_only: body.read_only.unwrap_or(false),
            },
            now_ms(),
        )
        .and_then(|repository| {
            api.issue_initial_token(account, &namespace, &body.name, now_ms())
                .map(|token| created_repository_dto(api, &namespace, &repository, token))
        });
    match result {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn list_repositories(
    State(state): State<HttpState>,
    Path((account_id, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match prepare(&state, &account_id, &request, V4Permission::Read, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let Ok(query) = parse_query::<RepositoriesQuery>(&request) else {
        return invalid_response(context.request_id());
    };
    match api
        .list_repositories(account, &namespace)
        .and_then(|records| {
            records
                .iter()
                .map(|record| repository_dto(api, &namespace, record))
                .collect::<Result<Vec<_>, _>>()
        }) {
        Ok(mut result) => {
            if let Some(search) = query.search.as_deref() {
                result.retain(|repo| repo.name.contains(search));
            }
            let ascending = match query.direction.as_deref().unwrap_or("desc") {
                "asc" => true,
                "desc" => false,
                _ => return invalid_response(context.request_id()),
            };
            match query.sort.as_deref().unwrap_or("created_at") {
                "name" => result.sort_by(|a, b| a.name.cmp(&b.name)),
                "created_at" => result.sort_by(|a, b| a.created_at.cmp(&b.created_at)),
                "updated_at" => result.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
                "last_push_at" => result.sort_by(|a, b| a.last_push_at.cmp(&b.last_push_at)),
                _ => return invalid_response(context.request_id()),
            }
            if !ascending {
                result.reverse();
            }
            cursor_page(
                context,
                result,
                query.limit,
                query.cursor.as_deref(),
                query.page,
            )
        }
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn get_repository(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match prepare(&state, &account_id, &request, V4Permission::Read, false) {
            Ok(value) => value,
            Err(response) => return response,
        };
    match api
        .repository(account, &namespace, &repository)
        .and_then(|record| repository_dto(api, &namespace, &record))
    {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn delete_repository(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match api.delete_repository(account, &namespace, &repository, now_ms()) {
        Ok(id) => {
            let mut response = success_response(context, serde_json::json!({"id": id}));
            *response.status_mut() = StatusCode::ACCEPTED;
            response
        }
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn fork_repository(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(body) = json::<ForkBody>(request, context.request_id()).await else {
        return invalid_response(context.request_id());
    };
    let result = api
        .fork_repository(
            account,
            &namespace,
            ForkRepositoryRequest {
                source_name: &repository,
                target_name: &body.name,
                description: body.description.as_deref(),
                read_only: body.read_only,
                default_branch_only: body.default_branch_only.unwrap_or(true),
            },
            now_ms(),
        )
        .and_then(|record| {
            let objects = api.object_count(record.id).map_err(|error| {
                api.abandon_created_repository(account, &namespace, &body.name, now_ms(), error)
            })?;
            let token = api.issue_initial_token(account, &namespace, &body.name, now_ms())?;
            Ok(ForkedRepositoryDto {
                repository: created_repository_dto(api, &namespace, &record, token),
                objects,
            })
        });
    match result {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn import_repository(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(body) = json::<ImportBody>(request, context.request_id()).await else {
        return invalid_response(context.request_id());
    };
    let result = api
        .import_repository(ImportRepositoryRequest {
            account,
            namespace: namespace.clone(),
            name: repository.clone(),
            remote: body.url,
            branch: body.branch,
            depth: body.depth,
            description: String::new(),
            read_only: body.read_only.unwrap_or(false),
            now_ms: now_ms(),
        })
        .await
        .and_then(|record| {
            let token = api.issue_initial_token(account, &namespace, &repository, now_ms())?;
            Ok(created_repository_dto(api, &namespace, &record, token))
        });
    match result {
        Ok(result) => success_response(context, result),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn issue_token(
    State(state): State<HttpState>,
    Path((account_id, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(body) = json::<IssueTokenBody>(request, context.request_id()).await else {
        return invalid_response(context.request_id());
    };
    let scope = match body.scope.as_deref().unwrap_or("write") {
        "read" => ArtifactTokenScope::Read,
        "write" => ArtifactTokenScope::Write,
        _ => return invalid_response(context.request_id()),
    };
    match api
        .issue_token(account, &namespace, &body.repo, scope, body.ttl, now_ms())
        .and_then(token_dto_once)
    {
        Ok(result) => success_response(context, result),
        Err(error) if error.code() == ErrorCode::LimitInvalid => {
            artifact_error_response(V4OfficialError::ArtifactInvalidTtl, context.request_id())
        }
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn list_tokens(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match prepare(&state, &account_id, &request, V4Permission::Read, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let Ok(query) = parse_query::<TokensQuery>(&request) else {
        return invalid_response(context.request_id());
    };
    match api
        .list_tokens(account, &namespace, &repository)
        .and_then(|tokens| {
            tokens
                .iter()
                .map(|token| token_dto(token, true))
                .collect::<Result<Vec<_>, _>>()
        }) {
        Ok(mut result) => {
            let state = query.state.as_deref().unwrap_or("active");
            if !matches!(state, "active" | "expired" | "revoked" | "all") {
                return invalid_response(context.request_id());
            }
            if state != "all" {
                result.retain(|token| token.state == Some(state));
            }
            offset_page(context, result, query.per_page, query.page)
        }
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

async fn revoke_token(
    State(state): State<HttpState>,
    Path((account_id, namespace, token)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        V4Permission::ProductWrite,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(token) = token.parse::<ArtifactTokenId>() else {
        return artifact_error_response(V4OfficialError::ArtifactNotFound, context.request_id());
    };
    match api.revoke_namespace_token(account, &namespace, token, now_ms()) {
        Ok(()) => success_response(context, serde_json::json!({"id": token})),
        Err(error) => platform_error_response(&error, context.request_id()),
    }
}

pub(super) fn prepare<'a>(
    state: &'a HttpState,
    public_account: &str,
    request: &Request,
    permission: V4Permission,
    allow_query: bool,
) -> Result<
    (
        super::V4RequestContext,
        open_compute_core::AccountId,
        &'a ArtifactApiState,
    ),
    Response,
> {
    let context = context(request, permission).map_err(super::wire::HttpError::into_response)?;
    if !allow_query {
        require_no_query(request).map_err(|_| invalid_response(context.request_id()))?;
    }
    let account = account(state, public_account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .artifact_api()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    Ok((context, account, api))
}

fn namespace_dto(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    record: &ArtifactNamespaceRecord,
) -> Result<NamespaceDto, PlatformError> {
    Ok(NamespaceDto {
        namespace: record.name.clone(),
        repo_count: api.list_repositories(account, &record.name)?.len(),
        jurisdiction: record.jurisdiction.clone(),
        created_at: iso_timestamp(record.created_at_ms).map_err(|_| dto_error())?,
        updated_at: iso_timestamp(record.updated_at_ms).map_err(|_| dto_error())?,
    })
}

fn repository_dto(
    api: &ArtifactApiState,
    namespace: &str,
    record: &ArtifactRepositoryRecord,
) -> Result<RepositoryDto, PlatformError> {
    Ok(RepositoryDto {
        id: record.id.to_string(),
        name: record.name.clone(),
        description: (!record.description.is_empty()).then(|| record.description.clone()),
        default_branch: record.default_branch.clone(),
        remote: api.remote(namespace, &record.name),
        read_only: record.read_only,
        created_at: iso_timestamp(record.created_at_ms).map_err(|_| dto_error())?,
        updated_at: iso_timestamp(record.updated_at_ms).map_err(|_| dto_error())?,
        last_push_at: record
            .last_push_at_ms
            .map(iso_timestamp)
            .transpose()
            .map_err(|_| dto_error())?,
        source: record.source.clone(),
    })
}

fn created_repository_dto(
    api: &ArtifactApiState,
    namespace: &str,
    record: &ArtifactRepositoryRecord,
    token: IssuedArtifactToken,
) -> CreatedRepositoryDto {
    CreatedRepositoryDto {
        id: record.id.to_string(),
        name: record.name.clone(),
        description: (!record.description.is_empty()).then(|| record.description.clone()),
        default_branch: record.default_branch.clone(),
        remote: api.remote(namespace, &record.name),
        token: token.plaintext,
    }
}
fn token_dto(record: &ArtifactTokenRecord, include_state: bool) -> Result<TokenDto, PlatformError> {
    let now = now_ms();
    Ok(TokenDto {
        id: record.id.to_string(),
        scope: record.scope,
        expires_at: iso_timestamp(record.expires_at_ms).map_err(|_| dto_error())?,
        state: include_state.then_some(if record.revoked_at_ms.is_some() {
            "revoked"
        } else if record.expires_at_ms <= now {
            "expired"
        } else {
            "active"
        }),
        created_at: include_state
            .then(|| iso_timestamp(record.created_at_ms).map_err(|_| dto_error()))
            .transpose()?,
        plaintext: None,
    })
}
fn token_dto_once(token: IssuedArtifactToken) -> Result<TokenDto, PlatformError> {
    let mut dto = token_dto(&token.record, false)?;
    dto.plaintext = Some(token.plaintext);
    Ok(dto)
}
pub(super) fn parse_query<T: DeserializeOwned>(request: &Request) -> Result<T, V4Error> {
    Query::<T>::try_from_uri(request.uri())
        .map(|value| value.0)
        .map_err(|_| V4Error::InvalidRequest)
}

pub(super) fn invalid_response(request_id: RequestId) -> Response {
    artifact_error_response(V4OfficialError::ArtifactInvalidInput, request_id)
}

pub(super) fn platform_error_response(error: &PlatformError, request_id: RequestId) -> Response {
    let error = match error.code() {
        ErrorCode::ResourceNotFound => V4OfficialError::ArtifactNotFound,
        ErrorCode::ResourceNameConflict => V4OfficialError::ArtifactAlreadyExists,
        ErrorCode::ArtifactUnavailable => V4OfficialError::ArtifactInvalidUrl,
        ErrorCode::BindingPermissionDenied => V4OfficialError::ArtifactRemoteAuthRequired,
        ErrorCode::ConfigInvalid => V4OfficialError::ArtifactInvalidRepoName,
        ErrorCode::PathInvalid | ErrorCode::LimitInvalid => V4OfficialError::ArtifactInvalidInput,
        ErrorCode::QuotaExceeded | ErrorCode::ResourceLimitExceeded => {
            V4OfficialError::ArtifactMemoryLimit
        }
        ErrorCode::ResourceNotReady | ErrorCode::ResourceUnavailable => {
            V4OfficialError::ArtifactUpstreamUnavailable
        }
        _ => V4OfficialError::ArtifactInternal,
    };
    artifact_error_response(error, request_id)
}

pub(super) fn artifact_error_response(error: V4OfficialError, request_id: RequestId) -> Response {
    error_response(V4Error::Official(error), request_id)
}

fn dto_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Artifact timestamp is invalid",
    )
}
