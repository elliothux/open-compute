//! Fixed-Wrangler Static Assets session, upload, and completion-token adapter.

use super::asset_wire::{
    base64_size, canonical_path, invalid, normalize_content_type, single_content_type, v4_error,
    valid_bulk_query, valid_multipart_content_type,
};
use super::domain;
use super::handlers::{authorize, now_ms, platform_error};
use super::model::WorkerUploadAssetsConfig;
use crate::cloudflare_v4::{
    V4Error, V4Permission, V4RequestContext, error_response, success_response,
};
use crate::http::HttpState;
use crate::workers_http::WorkerApiState;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use base64::Engine as _;
use bytes::Bytes;
use futures::stream;
use open_compute_core::{AccountId, ErrorCode, PlatformError};
use open_compute_storage::{AssetUploadRepository, AssetUploadSession, NewAssetUploadEntry};
use open_compute_workers::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, HtmlHandling, NotFoundHandling,
    RunWorkerFirst, VersionAssets, validate_asset_path,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

const SESSION_TTL_MS: i64 = 60 * 60 * 1000;
const MAX_OPEN_SESSIONS: u32 = 4;
const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
const JWT_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

pub(crate) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account}/workers/scripts/{script}/assets-upload-session",
            post(create_session),
        )
        .route(
            "/accounts/{account}/workers/assets/upload",
            post(upload_bulk),
        )
        .route(
            "/accounts/{account}/workers/assets/upload/{hash}",
            post(upload_single),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSessionBody {
    manifest: BTreeMap<String, ManifestValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestValue {
    hash: String,
    size: u64,
}

#[derive(Serialize)]
struct CreateSessionResult {
    buckets: Vec<Vec<String>>,
    jwt: String,
}

#[derive(Serialize)]
struct CompletedResult {
    jwt: String,
}

pub(super) struct AssetReservation {
    pub(super) session_id: String,
    pub(super) operation_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AssetTokenClaims {
    purpose: AssetTokenPurpose,
    session: String,
    account: String,
    script: String,
    exp: i64,
    wrangler_single_asset_uploads: bool,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AssetTokenPurpose {
    Upload,
    Complete,
}

async fn create_session(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let body =
        match super::json::json_body_with_limit::<CreateSessionBody>(request, 16 * 1024 * 1024)
            .await
        {
            Ok(value) => value,
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        };
    let account_id = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.worker_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let mut entries = match body
        .manifest
        .into_iter()
        .map(|(path, value)| {
            let path = canonical_path(&path);
            validate_asset_path(&path)?;
            Ok(NewAssetUploadEntry {
                path,
                wrangler_hash: value.hash,
                size: value.size,
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()
    {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    if entries
        .windows(2)
        .any(|pair| pair[0].path.as_bytes() >= pair[1].path.as_bytes())
    {
        return platform_error(context.request_id(), &invalid());
    }
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let expires = match now.checked_add(SESSION_TTL_MS) {
        Some(value) => value,
        None => return error_response(V4Error::Internal, context.request_id()),
    };
    let session_id = uuid::Uuid::now_v7().to_string();
    let session = AssetUploadRepository::new(api.storage.db()).create(
        &session_id,
        account_id,
        &script,
        &entries,
        now,
        expires,
        MAX_OPEN_SESSIONS,
    );
    let session = match session {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let missing = session
        .entries
        .iter()
        .filter(|entry| entry.artifact_sha256.is_none())
        .map(|entry| entry.wrangler_hash.clone())
        .collect::<BTreeSet<_>>();
    let buckets = bucket_hashes(&session, &missing);
    let purpose = if missing.is_empty() {
        AssetTokenPurpose::Complete
    } else {
        AssetTokenPurpose::Upload
    };
    let jwt = match issue_token(api, &session, purpose) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    success_response(context, CreateSessionResult { buckets, jwt })
}

async fn upload_bulk(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_bulk_query(request.uri().query()) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if !valid_multipart_content_type(&request) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let Some(api) = state.worker_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let account_id = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let claims = match upload_claims(&api, &request, account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let multipart = match Multipart::from_request(request, &state).await {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    upload_multipart(&api, context, claims, multipart).await
}

async fn upload_multipart(
    api: &WorkerApiState,
    context: V4RequestContext,
    claims: AssetTokenClaims,
    mut multipart: Multipart,
) -> Response {
    let account = match AccountId::from_str(&claims.account) {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let mut session = match current_session(api, &claims) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let mut total = 0_usize;
    while let Some(field) = match multipart.next_field().await {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    } {
        let Some(hash) = field.name().map(str::to_owned) else {
            return error_response(V4Error::InvalidRequest, context.request_id());
        };
        let content_type = normalize_content_type(field.content_type());
        let encoded = match field.bytes().await {
            Ok(value) => value,
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        };
        total = match total.checked_add(encoded.len()) {
            Some(value) if value <= MAX_UPLOAD_BYTES => value,
            _ => {
                return error_response(
                    V4Error::Official(crate::cloudflare_v4::V4OfficialError::RequestTooLarge),
                    context.request_id(),
                );
            }
        };
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&encoded) {
            Ok(value) => value,
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        };
        session = match persist_asset(
            api,
            &session,
            account,
            &claims.script,
            &hash,
            Some(&content_type),
            bytes,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return platform_error(context.request_id(), &error),
        };
    }
    upload_result(api, context, &session)
}

async fn upload_single(
    State(state): State<HttpState>,
    Path((account, hash)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let Some(api) = state.worker_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let account_id = match domain::resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let claims = match upload_claims(&api, &request, account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let content_type = match single_content_type(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let bytes = match to_bytes(request.into_body(), 25 * 1024 * 1024).await {
        Ok(value) => value.to_vec(),
        Err(_) => {
            return error_response(
                V4Error::Official(crate::cloudflare_v4::V4OfficialError::RequestTooLarge),
                context.request_id(),
            );
        }
    };
    let session = match current_session(&api, &claims) {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    let session = match persist_asset(
        &api,
        &session,
        account_id,
        &claims.script,
        &hash,
        Some(&content_type),
        bytes,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return platform_error(context.request_id(), &error),
    };
    upload_result(&api, context, &session)
}

async fn persist_asset(
    api: &WorkerApiState,
    session: &AssetUploadSession,
    account: AccountId,
    script: &str,
    hash: &str,
    content_type: Option<&str>,
    bytes: Vec<u8>,
) -> Result<AssetUploadSession, PlatformError> {
    let entries = session
        .entries
        .iter()
        .filter(|entry| entry.wrangler_hash == hash)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::AssetUploadConflict,
            "asset hash is outside the upload session",
        ));
    }
    let size = u64::try_from(bytes.len()).map_err(|_| invalid())?;
    if entries.iter().any(|entry| {
        entry.size != size || wrangler_hash(&bytes, &entry.path).as_str() != entry.wrangler_hash
    }) {
        return Err(PlatformError::new(
            ErrorCode::AssetUploadConflict,
            "asset content does not match the manifest hash and size",
        ));
    }
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let normalized_content_type = normalize_content_type(content_type);
    if entries.iter().any(|entry| {
        entry
            .artifact_sha256
            .is_some_and(|existing| existing != digest)
            || entry.artifact_sha256.is_some()
                && entry.content_type.as_deref() != Some(normalized_content_type.as_str())
    }) {
        return Err(PlatformError::new(
            ErrorCode::AssetUploadConflict,
            "asset retry does not match the already verified object",
        ));
    }
    if entries.iter().all(|entry| entry.artifact_sha256.is_some()) {
        return Ok(session.clone());
    }
    let _reservation = api.artifacts.reserve_version_artifact().await;
    api.artifacts
        .put_verified(
            stream::once(async move { Ok::<Bytes, std::io::Error>(Bytes::from(bytes)) }),
            &hex::encode(digest),
            size,
        )
        .await?;
    AssetUploadRepository::new(api.storage.db()).mark_uploaded(
        &session.id,
        account,
        script,
        hash,
        Some(&normalized_content_type),
        digest,
        size,
        now_ms().map_err(v4_error)?,
    )
}

fn wrangler_hash(bytes: &[u8], path: &str) -> String {
    let file_name = path.rsplit('/').next().map_or("", |value| value);
    let extension = file_name
        .rfind('.')
        .filter(|offset| *offset > 0)
        .map_or("", |offset| &file_name[offset + 1..]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut hasher = blake3::Hasher::new();
    hasher.update(encoded.as_bytes());
    hasher.update(extension.as_bytes());
    hasher.finalize().to_hex()[..32].to_owned()
}

fn upload_result(
    api: &WorkerApiState,
    context: V4RequestContext,
    session: &AssetUploadSession,
) -> Response {
    let mut response = if session.complete {
        match issue_token(api, session, AssetTokenPurpose::Complete) {
            Ok(jwt) => success_response(context, CompletedResult { jwt }),
            Err(error) => return platform_error(context.request_id(), &error),
        }
    } else {
        success_response(context, BTreeMap::<String, String>::new())
    };
    *response.status_mut() = if session.complete {
        StatusCode::CREATED
    } else {
        StatusCode::ACCEPTED
    };
    response
}

pub(super) fn redeem_assets(
    api: &WorkerApiState,
    token: &str,
    account_id: AccountId,
    script_name: &str,
    reservation_id: Option<&str>,
    binding: Option<String>,
    config: &WorkerUploadAssetsConfig,
    now_ms: i64,
) -> Result<(VersionAssets, AssetReservation), PlatformError> {
    let claims = open_token(api, token).map_err(v4_error)?;
    if claims.purpose != AssetTokenPurpose::Complete || claims.exp <= now_ms.div_euclid(1_000) {
        return Err(invalid());
    }
    let account = AccountId::from_str(&claims.account).map_err(|_| invalid())?;
    if account != account_id || claims.script != script_name {
        return Err(invalid());
    }
    let repository = AssetUploadRepository::new(api.storage.db());
    let session = if let Some(reservation_id) = reservation_id {
        repository.reserve(
            &claims.session,
            account,
            script_name,
            reservation_id,
            now_ms,
        )?
    } else {
        repository.get(&claims.session, account, script_name, now_ms)?
    };
    if !session.complete {
        return Err(PlatformError::new(
            ErrorCode::AssetUploadIncomplete,
            "Static Assets upload is incomplete",
        ));
    }
    let manifest = AssetManifestV1 {
        schema_version: 1,
        entries: session
            .entries
            .into_iter()
            .map(|entry| {
                Ok(AssetEntryV1 {
                    path: entry.path,
                    sha256: hex::encode(entry.artifact_sha256.ok_or_else(invalid)?),
                    size: entry.size,
                    content_type: entry
                        .content_type
                        .unwrap_or_else(|| "application/octet-stream".to_owned()),
                })
            })
            .collect::<Result<Vec<_>, PlatformError>>()?,
    };
    let routing = routing_config(binding, config)?;
    manifest.validate()?;
    routing.validate()?;
    Ok((
        VersionAssets { manifest, routing },
        AssetReservation {
            session_id: session.id,
            operation_id: session.reservation_id,
        },
    ))
}

pub(super) fn consume_assets(
    api: &WorkerApiState,
    reservation: &AssetReservation,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let operation = reservation.operation_id.as_deref().ok_or_else(invalid)?;
    AssetUploadRepository::new(api.storage.db()).consume(&reservation.session_id, operation, now_ms)
}

pub(super) fn release_assets(
    api: &WorkerApiState,
    reservation: &AssetReservation,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let operation = reservation.operation_id.as_deref().ok_or_else(invalid)?;
    AssetUploadRepository::new(api.storage.db()).release(&reservation.session_id, operation, now_ms)
}

fn routing_config(
    binding: Option<String>,
    config: &WorkerUploadAssetsConfig,
) -> Result<AssetRoutingConfigV1, PlatformError> {
    if config
        ._headers
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || config
            ._redirects
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        return Err(PlatformError::new(
            ErrorCode::AssetConfigUnsupported,
            "Static Assets _headers and _redirects are not supported by this release",
        ));
    }
    let run_worker_first = match &config.run_worker_first {
        None => RunWorkerFirst::default(),
        Some(serde_json::Value::Bool(value)) => RunWorkerFirst::All(*value),
        Some(serde_json::Value::Array(values)) => RunWorkerFirst::Rules(
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err(invalid()),
    };
    let html_handling = match config.html_handling.as_deref() {
        None | Some("auto-trailing-slash") => HtmlHandling::AutoTrailingSlash,
        Some("force-trailing-slash") => HtmlHandling::ForceTrailingSlash,
        Some("drop-trailing-slash") => HtmlHandling::DropTrailingSlash,
        Some("none") => HtmlHandling::None,
        Some(_) => return Err(invalid()),
    };
    let not_found_handling = match config.not_found_handling.as_deref() {
        None | Some("none") => NotFoundHandling::None,
        Some("404-page") => NotFoundHandling::Page404,
        Some("single-page-application") => NotFoundHandling::SinglePageApplication,
        Some(_) => return Err(invalid()),
    };
    Ok(AssetRoutingConfigV1 {
        schema_version: 1,
        binding,
        run_worker_first,
        html_handling,
        not_found_handling,
        headers: Vec::new(),
        redirects: Vec::new(),
    })
}

pub(crate) fn authenticate_upload_token(
    state: &HttpState,
    path: &str,
    bearer: Option<&str>,
) -> bool {
    let segments = path
        .split('/')
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let account = match segments.as_slice() {
        [
            "client",
            "v4",
            "accounts",
            account,
            "workers",
            "assets",
            "upload",
        ]
        | [
            "client",
            "v4",
            "accounts",
            account,
            "workers",
            "assets",
            "upload",
            _,
        ]
        | ["accounts", account, "workers", "assets", "upload"]
        | ["accounts", account, "workers", "assets", "upload", _] => *account,
        _ => return false,
    };
    let Some(api) = state.worker_api() else {
        return false;
    };
    let Ok(account) = domain::resolve_account(state, account) else {
        return false;
    };
    let Some(token) = bearer else {
        return false;
    };
    let Ok(now) = unix_seconds() else {
        return false;
    };
    open_token(api, token).is_ok_and(|claims| {
        claims.purpose == AssetTokenPurpose::Upload
            && claims.account == account.to_string()
            && claims.exp > now
    })
}

fn upload_claims(
    api: &WorkerApiState,
    request: &Request,
    account: AccountId,
) -> Result<AssetTokenClaims, V4Error> {
    let token = bearer(request).ok_or(V4Error::AuthenticationRequired)?;
    let claims = open_token(api, token)?;
    if claims.purpose != AssetTokenPurpose::Upload
        || claims.account != account.to_string()
        || claims.exp <= unix_seconds()?
    {
        return Err(V4Error::AuthenticationRequired);
    }
    Ok(claims)
}

fn current_session(
    api: &WorkerApiState,
    claims: &AssetTokenClaims,
) -> Result<AssetUploadSession, PlatformError> {
    AssetUploadRepository::new(api.storage.db()).get(
        &claims.session,
        AccountId::from_str(&claims.account).map_err(|_| invalid())?,
        &claims.script,
        now_ms().map_err(v4_error)?,
    )
}

fn unix_seconds() -> Result<i64, V4Error> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| V4Error::Internal)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| V4Error::Internal)
}

fn issue_token(
    api: &WorkerApiState,
    session: &AssetUploadSession,
    purpose: AssetTokenPurpose,
) -> Result<String, PlatformError> {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(JWT_HEADER);
    let claims = AssetTokenClaims {
        purpose,
        session: session.id.clone(),
        account: session.account_id.to_string(),
        script: session.script_name.clone(),
        exp: session.expires_at_ms / 1000,
        wrangler_single_asset_uploads: false,
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).map_err(|_| invalid())?);
    let signed = format!("{header}.{payload}");
    let signature = api
        .storage
        .crypto()
        .sign_asset_upload_token(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn open_token(api: &WorkerApiState, token: &str) -> Result<AssetTokenClaims, V4Error> {
    let mut parts = token.split('.');
    let header = parts.next().ok_or(V4Error::AuthenticationRequired)?;
    let payload = parts.next().ok_or(V4Error::AuthenticationRequired)?;
    let signature = parts.next().ok_or(V4Error::AuthenticationRequired)?;
    if parts.next().is_some() {
        return Err(V4Error::AuthenticationRequired);
    }
    let signed = format!("{header}.{payload}");
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| V4Error::AuthenticationRequired)?;
    if !api
        .storage
        .crypto()
        .verify_asset_upload_token(signed.as_bytes(), &signature)
    {
        return Err(V4Error::AuthenticationRequired);
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| V4Error::AuthenticationRequired)?;
    serde_json::from_slice(&payload).map_err(|_| V4Error::AuthenticationRequired)
}

fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn bucket_hashes(session: &AssetUploadSession, missing: &BTreeSet<String>) -> Vec<Vec<String>> {
    let mut buckets = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 0_u64;
    for hash in missing {
        let size = session
            .entries
            .iter()
            .find(|entry| &entry.wrangler_hash == hash)
            .map_or(0, |entry| base64_size(entry.size));
        if !current.is_empty() && bytes.saturating_add(size) > MAX_UPLOAD_BYTES as u64 {
            buckets.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(hash.clone());
        bytes = bytes.saturating_add(size);
    }
    if !current.is_empty() {
        buckets.push(current);
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_wrangler_hash_is_content_and_extension_sensitive() {
        let html = wrangler_hash(b"good", "/index.html");
        assert_eq!(html, "4c73266e449fea54bba5a6dea074dbbd");
        assert_ne!(html, wrangler_hash(b"bad!", "/index.html"));
        assert_ne!(html, wrangler_hash(b"good", "/index.txt"));
        assert_eq!(
            wrangler_hash(b"good", "/.well-known"),
            wrangler_hash(b"good", "/extensionless")
        );
    }
}
