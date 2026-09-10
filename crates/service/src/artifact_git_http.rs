//! Git Smart HTTP data plane for Cloudflare Artifacts repositories.

use crate::http::HttpState;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use futures::TryStreamExt as _;
use gitserver_core::backend::GitBackend;
use http_body_util::{BodyExt as _, Limited};
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::io::{ReaderStream, StreamReader};

const MAX_RECEIVE_COMMAND_BYTES: usize = 64 * 1024;
const MAX_RECEIVE_COMMANDS: usize = 256;

pub(crate) fn router() -> Router<HttpState> {
    Router::new()
        .route("/git/{namespace}/{repository}/info/refs", get(info_refs))
        .route(
            "/git/{namespace}/{repository}/git-upload-pack",
            post(upload_pack),
        )
        .route(
            "/git/{namespace}/{repository}/git-receive-pack",
            post(receive_pack),
        )
}

#[derive(Deserialize)]
struct DiscoveryQuery {
    service: String,
}

async fn info_refs(
    State(state): State<HttpState>,
    Path((namespace, repository)): Path<(String, String)>,
    Query(query): Query<DiscoveryQuery>,
    headers: HeaderMap,
) -> Response {
    let receive = query.service == "git-receive-pack";
    if !receive && query.service != "git-upload-pack" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some((api, repository, _permit, _lease)) =
        authorize(&state, &namespace, &repository, &headers, receive)
    else {
        return unauthorized();
    };
    if receive && repository.read_only {
        return StatusCode::FORBIDDEN.into_response();
    }
    let backend = GitBackend::new(api.git().path(repository.id));
    let protocol_v2 = !receive && is_protocol_v2(&headers);
    let body = if protocol_v2 {
        gitserver_core::protocol_v2::advertise_capabilities()
    } else if receive {
        let mut body = gitserver_core::pktline::encode_comment("service=git-receive-pack");
        body.extend_from_slice(gitserver_core::pktline::flush());
        match backend.advertise_receive_refs() {
            Ok(refs) => {
                body.extend_from_slice(&refs);
                body
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    } else {
        match backend.advertise_refs() {
            Ok(body) => body,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            format!("application/x-{}-advertisement", query.service),
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn upload_pack(
    State(state): State<HttpState>,
    Path((namespace, repository)): Path<(String, String)>,
    request: Request,
) -> Response {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/x-git-upload-pack-request")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let headers = request.headers().clone();
    let Some((api, repository, permit, _lease)) =
        authorize(&state, &namespace, &repository, &headers, false)
    else {
        return unauthorized();
    };
    let Ok(limit) = usize::try_from(api.max_request_bytes()) else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(body) = to_bytes(request.into_body(), limit).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let backend = GitBackend::new(api.git().path(repository.id));
    let result = if is_protocol_v2(&headers) {
        upload_pack_v2(api, repository.id, &backend, &body).await
    } else {
        let Ok(parsed) = gitserver_core::pack::UploadPackRequest::parse(&body) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let wants = parsed
            .wants
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if api
            .git()
            .validate_advertised_wants(repository.id, &wants)
            .is_err()
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        match backend.upload_pack(&parsed).await {
            Ok(reader) => git_response(ReaderStream::new(reader)),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    drop(permit);
    result
}

async fn receive_pack(
    State(state): State<HttpState>,
    Path((namespace, repository)): Path<(String, String)>,
    request: Request,
) -> Response {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/x-git-receive-pack-request")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let headers = request.headers().clone();
    let Some((api, repository, permit, _lease)) =
        authorize(&state, &namespace, &repository, &headers, true)
    else {
        return unauthorized();
    };
    if repository.read_only {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(_disk) = api.admit_git_mutation(repository.id) else {
        return StatusCode::INSUFFICIENT_STORAGE.into_response();
    };
    let Ok(limit) = usize::try_from(api.max_request_bytes()) else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let backend = GitBackend::new(api.git().path(repository.id));
    let exceeded = Arc::new(AtomicBool::new(false));
    let exceeded_on_error = Arc::clone(&exceeded);
    let stream = Limited::new(request.into_body(), limit)
        .into_data_stream()
        .map_err(move |error| {
            if caused_by_length_limit(&*error) {
                exceeded_on_error.store(true, Ordering::Relaxed);
            }
            std::io::Error::other(error)
        });
    let mut reader = StreamReader::new(stream);
    let commands = match inspect_receive_commands(&mut reader).await {
        Ok(value) => value,
        Err(()) if exceeded.load(Ordering::Relaxed) => {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        Err(()) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let reader = tokio::io::AsyncReadExt::chain(std::io::Cursor::new(commands), reader);
    let response = match backend.receive_pack(reader).await {
        Ok(body) => {
            let _ = api.record_push(repository.id, open_compute_core::wall_time_ms());
            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_TYPE,
                    "application/x-git-receive-pack-result",
                )
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) if exceeded.load(Ordering::Relaxed) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    };
    drop(permit);
    response
}

async fn inspect_receive_commands<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Vec<u8>, ()> {
    use tokio::io::AsyncReadExt as _;

    let mut encoded = Vec::new();
    let mut count = 0_usize;
    loop {
        let mut prefix = [0_u8; 4];
        reader.read_exact(&mut prefix).await.map_err(|_| ())?;
        encoded.extend_from_slice(&prefix);
        if &prefix == b"0000" {
            return Ok(encoded);
        }
        let length = std::str::from_utf8(&prefix)
            .ok()
            .and_then(|value| usize::from_str_radix(value, 16).ok())
            .filter(|value| *value >= 4)
            .ok_or(())?;
        if encoded.len().saturating_add(length - 4) > MAX_RECEIVE_COMMAND_BYTES
            || count >= MAX_RECEIVE_COMMANDS
        {
            return Err(());
        }
        let mut payload = vec![0_u8; length - 4];
        reader.read_exact(&mut payload).await.map_err(|_| ())?;
        validate_receive_command(&payload)?;
        encoded.extend_from_slice(&payload);
        count += 1;
    }
}

fn validate_receive_command(payload: &[u8]) -> Result<(), ()> {
    let command = payload.split(|byte| *byte == 0).next().ok_or(())?;
    let command = std::str::from_utf8(command).map_err(|_| ())?;
    let mut fields = command.trim_end_matches('\n').split_ascii_whitespace();
    let old = fields.next().ok_or(())?;
    let new = fields.next().ok_or(())?;
    let reference = fields.next().ok_or(())?;
    if fields.next().is_some()
        || !valid_object_id(old)
        || !valid_object_id(new)
        || !valid_ref_name(reference)
    {
        return Err(());
    }
    Ok(())
}

fn valid_ref_name(reference: &str) -> bool {
    (reference.starts_with("refs/heads/") || reference.starts_with("refs/tags/"))
        && !reference.contains("..")
        && !reference.contains("@{")
        && !reference.contains("//")
        && !reference.ends_with('/')
        && !reference.ends_with('.')
        && reference.split('/').all(|component| {
            !component.is_empty()
                && !component.starts_with('.')
                && !component.ends_with(".lock")
                && !component.bytes().any(|byte| {
                    byte <= b' '
                        || byte == 0x7f
                        || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
                })
        })
}

fn caused_by_length_limit(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        current = error.source();
    }
    false
}

fn valid_object_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn authorize<'a>(
    state: &'a HttpState,
    namespace: &str,
    repository: &str,
    headers: &HeaderMap,
    write: bool,
) -> Option<(
    &'a crate::artifact_api::ArtifactApiState,
    open_compute_storage::ArtifactRepositoryRecord,
    tokio::sync::OwnedSemaphorePermit,
    crate::artifact_api::leases::RepositoryLease,
)> {
    let repository = repository.strip_suffix(".git")?;
    let api = state.artifact_api()?;
    let account = state.cloudflare_v4_account()?.internal_id();
    let (record, lease) = api
        .repository_with_lease(account, namespace, repository)
        .ok()?;
    let token = authorization_token(headers)?;
    api.authenticate_git(&record, &token, write, open_compute_core::wall_time_ms())
        .ok()?;
    let permit = api.admit_git().ok()?;
    Some((api, record, permit, lease))
}

fn authorization_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    if let Some(token) = value.strip_prefix("Bearer ") {
        return Some(token.to_owned());
    }
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded
        .split_once(':')
        .map(|(_, password)| password.to_owned())
}

fn is_protocol_v2(headers: &HeaderMap) -> bool {
    headers
        .get("git-protocol")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(':').any(|part| part.trim() == "version=2"))
}

async fn upload_pack_v2(
    api: &crate::artifact_api::ArtifactApiState,
    repository: open_compute_core::ArtifactRepoId,
    backend: &GitBackend,
    request: &[u8],
) -> Response {
    let path = api.git().path(repository);
    match gitserver_core::protocol_v2::parse_command_request(request) {
        Ok(gitserver_core::protocol_v2::Command::LsRefs(value)) => {
            match gitserver_core::protocol_v2::ls_refs(&path, &value) {
                Ok(body) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(Body::from(body))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                Err(_) => StatusCode::BAD_REQUEST.into_response(),
            }
        }
        Ok(gitserver_core::protocol_v2::Command::Fetch(mut value)) => {
            let wants = value
                .upload_request
                .wants
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if api
                .git()
                .validate_advertised_wants(repository, &wants)
                .is_err()
            {
                return StatusCode::BAD_REQUEST.into_response();
            }
            let Ok(shallow) =
                gitserver_core::protocol_v2::apply_shallow_boundaries(&path, &mut value)
            else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let shallow_negotiation = value.upload_request.shallow.depth.is_some();
            if !value.upload_request.done && !shallow_negotiation {
                let Ok(common) = gitserver_core::protocol_v2::common_haves(&path, &value) else {
                    return StatusCode::BAD_REQUEST.into_response();
                };
                let mut body = gitserver_core::protocol_v2::encode_fetch_acknowledgments(&common);
                body.extend_from_slice(&gitserver_core::protocol_v2::encode_shallow_info(&shallow));
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
                    .body(Body::from(body))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
            let Ok(reader) = backend.upload_pack(&value.upload_request).await else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            let mut prefix = gitserver_core::protocol_v2::encode_shallow_info(&shallow);
            prefix.extend_from_slice(&gitserver_core::pktline::encode(b"packfile\n"));
            git_response(ReaderStream::new(
                gitserver_core::protocol_v2::PrefixThenReader::new(
                    prefix,
                    gitserver_core::protocol_v2::PackSectionReader::new(reader),
                ),
            ))
        }
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

fn git_response(
    stream: impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send + 'static,
) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-git-upload-pack-result")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn unauthorized() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"Artifacts\", charset=\"UTF-8\"",
        )
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::UNAUTHORIZED.into_response())
}

#[cfg(test)]
mod tests;
