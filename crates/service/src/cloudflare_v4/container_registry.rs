//! Authenticated OCI distribution ingress; execution never pulls from this surface.

use crate::{
    container_images::ContainerImageUpload, containers::ContainerService, http::HttpState,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt as _;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef, ContainerImageLimits};
use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use open_compute_storage::containers::{
    ContainerAuthority, ContainerRegistryBlob, validate_registry_tag,
};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeSet, sync::Arc};
use tokio::io::AsyncWriteExt as _;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route("/v2/", axum::routing::any(handshake))
        .route("/v2/{*path}", axum::routing::any(handle))
}

pub(crate) fn password_hash(password: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"dev.open-compute.container.registry-token.v1\0");
    digest.update(password.as_bytes());
    digest.finalize().into()
}

fn authenticate(
    state: &HttpState,
    request: &Request,
    repository: Option<&str>,
) -> Result<Arc<ContainerService>, Response> {
    let service = state
        .container_service()
        .cloned()
        .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"))?;
    let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
    let value = headers
        .next()
        .and_then(|h| h.to_str().ok())
        .filter(|_| headers.next().is_none())
        .ok_or_else(unauthorized)?;
    let (username, password) = if let Some(encoded) = value.strip_prefix("Basic ") {
        let decoded = STANDARD.decode(encoded).map_err(|_| unauthorized())?;
        let decoded = String::from_utf8(decoded).map_err(|_| unauthorized())?;
        let (username, password) = decoded.split_once(':').ok_or_else(unauthorized)?;
        (username.to_owned(), password.to_owned())
    } else if let Some(password) = value.strip_prefix("Bearer ") {
        (service.instance_id().to_string(), password.to_owned())
    } else {
        return Err(unauthorized());
    };
    if username != service.instance_id().as_str() || password.len() != 64 {
        return Err(unauthorized());
    }
    let repository = repository.map_or_else(
        || format!("{}/handshake", service.instance_id()),
        str::to_owned,
    );
    let push = !matches!(*request.method(), Method::GET | Method::HEAD);
    let authenticated = ContainerAuthority::new(service.storage.db())
        .authenticate_registry(
            service.instance_id(),
            &password_hash(&password),
            &repository,
            push,
            open_compute_core::wall_time_ms(),
        )
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"))?;
    if !authenticated {
        return Err(unauthorized());
    }
    Ok(service)
}

async fn handshake(State(state): State<HttpState>, request: Request) -> Response {
    match authenticate(&state, &request, None) {
        Ok(_) => reply(StatusCode::OK, Body::from("{}"), None, None),
        Err(response) => response,
    }
}

async fn handle(
    State(state): State<HttpState>,
    Path(path): Path<String>,
    request: Request,
) -> Response {
    let split = path
        .split_once("/blobs/")
        .map(|(repo, rest)| (repo, rest, false))
        .or_else(|| {
            path.split_once("/manifests/")
                .map(|(repo, rest)| (repo, rest, true))
        });
    let Some((repository, rest, manifest)) = split else {
        return failure(StatusCode::NOT_FOUND, "NAME_UNKNOWN");
    };
    let service = match authenticate(&state, &request, Some(repository)) {
        Ok(service) => service,
        Err(response) => return response,
    };
    let result = if manifest {
        manifest_request(&service, repository, rest, request).await
    } else {
        blob_request(&service, repository, rest, request).await
    };
    match result {
        Ok(response) => response,
        Err(error) => {
            let status = if error.code() == ErrorCode::ResourceNotFound {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            failure(status, "DENIED")
        }
    }
}

async fn blob_request(
    service: &ContainerService,
    repository: &str,
    rest: &str,
    request: Request,
) -> Result<Response, PlatformError> {
    let repo = ContainerAuthority::new(service.storage.db());
    if let Some(upload) = rest.strip_prefix("uploads/") {
        if request.method() == Method::POST && upload.is_empty() {
            let query = super::strict_query(&request).map_err(|_| invalid())?;
            if !query.is_empty() {
                if query.len() != 2 {
                    return Err(invalid());
                }
                let digest = query
                    .get("mount")
                    .ok_or_else(invalid)
                    .and_then(|value| parse_digest(value))?;
                let from = query.get("from").ok_or_else(invalid)?;
                if repo.mount_registry_blob(
                    service.instance_id(),
                    from,
                    repository,
                    &digest,
                    open_compute_core::wall_time_ms(),
                )? {
                    to_bytes(request.into_body(), 0)
                        .await
                        .map_err(|_| invalid())?;
                    return Ok(reply(
                        StatusCode::CREATED,
                        Body::empty(),
                        Some(format!(
                            "/v2/{repository}/blobs/sha256:{}",
                            hex::encode(digest)
                        )),
                        Some(digest),
                    ));
                }
            }
            to_bytes(request.into_body(), 0)
                .await
                .map_err(|_| invalid())?;
            let id = ResourceId::generate();
            repo.create_registry_upload(
                service.instance_id(),
                id,
                repository,
                open_compute_core::wall_time_ms() + 900_000,
            )?;
            let path = upload_path(service, id)?;
            let file = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
                .await
                .map_err(|_| invalid())?;
            file.sync_all().await.map_err(|_| invalid())?;
            return upload_reply(repository, id, 0, StatusCode::ACCEPTED);
        }
        let id: ResourceId = upload.parse().map_err(|_| invalid())?;
        if !matches!(*request.method(), Method::PATCH | Method::PUT) {
            return Err(invalid());
        }
        let expected = if request.method() == Method::PUT {
            let query = super::strict_query(&request).map_err(|_| invalid())?;
            if query.len() != 1 {
                return Err(invalid());
            }
            Some(parse_digest(query.get("digest").ok_or_else(invalid)?)?)
        } else {
            if request.uri().query().is_some() {
                return Err(invalid());
            }
            None
        };
        let prior = repo.claim_registry_upload(
            service.instance_id(),
            id,
            repository,
            open_compute_core::wall_time_ms(),
        )?;
        let path = upload_path(service, id)?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.len() != prior {
            return Err(invalid());
        }
        let _disk = service.storage.reserve_mutation(
            service
                .registry_request_bytes()
                .checked_sub(prior)
                .ok_or_else(invalid)?,
        )?;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(|_| invalid())?;
        let mut stream = request.into_body().into_data_stream();
        let mut size = prior;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| invalid())?;
            size = size.checked_add(chunk.len() as u64).ok_or_else(invalid)?;
            if size > service.registry_request_bytes() {
                return Err(invalid());
            }
            file.write_all(&chunk).await.map_err(|_| invalid())?;
        }
        file.sync_all().await.map_err(|_| invalid())?;
        drop(file);
        repo.finish_registry_append(service.instance_id(), id, prior, size)?;
        let Some(expected) = expected else {
            return upload_reply(repository, id, size, StatusCode::ACCEPTED);
        };
        let size = repo.claim_registry_upload(
            service.instance_id(),
            id,
            repository,
            open_compute_core::wall_time_ms(),
        )?;
        let _gc = service.artifacts.reserve_version_artifact().await;
        let artifact = service
            .artifacts
            .put_verified_file(&path, &hex::encode(expected), size)
            .await?;
        repo.commit_registry_blob(
            service.instance_id(),
            id,
            repository,
            &ContainerRegistryBlob {
                sha256: *artifact.sha256_bytes(),
                size: artifact.size(),
            },
            open_compute_core::wall_time_ms(),
        )?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid()),
        }
        return Ok(reply(
            StatusCode::CREATED,
            Body::empty(),
            Some(format!(
                "/v2/{repository}/blobs/sha256:{}",
                hex::encode(expected)
            )),
            Some(expected),
        ));
    }
    if request.uri().query().is_some() || !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(invalid());
    }
    let digest = parse_digest(rest)?;
    let blob = repo
        .registry_blobs(service.instance_id(), repository)?
        .into_iter()
        .find(|blob| blob.sha256 == digest)
        .ok_or_else(not_found)?;
    let body = if request.method() == Method::HEAD {
        Body::empty()
    } else {
        let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), blob.size)?;
        let pin = service.cache.acquire(&service.artifacts, &artifact).await?;
        let file = tokio::fs::File::from_std(pin.file().try_clone().map_err(|_| invalid())?);
        let stream = tokio_util::io::ReaderStream::new(file).map(move |chunk| {
            let _ = &pin;
            chunk
        });
        Body::from_stream(stream)
    };
    let mut response = reply(StatusCode::OK, body, None, Some(digest));
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        blob.size.to_string().parse().map_err(|_| invalid())?,
    );
    Ok(response)
}

async fn manifest_request(
    service: &ContainerService,
    repository: &str,
    reference: &str,
    request: Request,
) -> Result<Response, PlatformError> {
    if request.uri().query().is_some() {
        return Err(invalid());
    }
    let repo = ContainerAuthority::new(service.storage.db());
    if request.method() == Method::PUT {
        let referenced_digest = if reference.starts_with("sha256:") {
            Some(parse_digest(reference)?)
        } else {
            validate_registry_tag(reference)?;
            None
        };
        let bytes = to_bytes(request.into_body(), 1 << 20)
            .await
            .map_err(|_| invalid())?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let expected: [u8; 32] = Sha256::digest(&bytes).into();
        if referenced_digest.is_some_and(|digest| digest != expected) {
            return Err(invalid());
        }
        let available = repo.registry_blobs(service.instance_id(), repository)?;
        let mut blobs = Vec::new();
        let mut referenced = BTreeSet::new();
        for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
            let sha = parse_digest(&descriptor.digest)?;
            let blob = available
                .iter()
                .find(|blob| blob.sha256 == sha && blob.size == descriptor.size)
                .ok_or_else(not_found)?;
            if referenced.insert(sha) {
                blobs.push(ArtifactRef::new(
                    ARTIFACT_KEY_VERSION,
                    &hex::encode(sha),
                    blob.size,
                )?);
            }
        }
        let config = blobs.first().ok_or_else(invalid)?;
        if config.size() > 1 << 20 {
            return Err(invalid());
        }
        let architecture = service.provider.architecture();
        let limits = ContainerImageLimits {
            json_bytes: 1 << 20,
            compressed_bytes: service.registry_request_bytes(),
            unpacked_bytes: service.registry_request_bytes(),
            layers: 128,
            entries: 100_000,
        };
        service
            .admit_image(ContainerImageUpload {
                manifest: &bytes,
                expected,
                architecture,
                blobs,
                limits,
            })
            .await?;
        repo.publish_registry_manifest(service.instance_id(), repository, &expected, &bytes)?;
        if referenced_digest.is_none() {
            repo.publish_registry_tag(
                service.instance_id(),
                repository,
                reference,
                &expected,
                open_compute_core::wall_time_ms(),
            )?;
        }
        return Ok(reply(
            StatusCode::CREATED,
            Body::empty(),
            Some(format!(
                "/v2/{repository}/manifests/sha256:{}",
                hex::encode(expected)
            )),
            Some(expected),
        ));
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(invalid());
    }
    let digest = if reference.starts_with("sha256:") {
        parse_digest(reference)?
    } else {
        repo.registry_tag(service.instance_id(), repository, reference)?
    };
    let content = repo.registry_manifest(service.instance_id(), repository, &digest)?;
    let size = content.len();
    let media: Manifest = serde_json::from_slice(&content).map_err(|_| invalid())?;
    let body = if request.method() == Method::HEAD {
        Body::empty()
    } else {
        Body::from(content)
    };
    let mut response = reply(StatusCode::OK, body, None, Some(digest));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        media.media_type.parse().map_err(|_| invalid())?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        size.to_string().parse().map_err(|_| invalid())?,
    );
    Ok(response)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    media_type: String,
    config: Descriptor,
    layers: Vec<Descriptor>,
}
#[derive(serde::Deserialize)]
struct Descriptor {
    digest: String,
    size: u64,
}

fn upload_path(
    service: &ContainerService,
    id: ResourceId,
) -> Result<std::path::PathBuf, PlatformError> {
    service.registry_upload_path(id)
}
fn parse_digest(value: &str) -> Result<[u8; 32], PlatformError> {
    let value = value.strip_prefix("sha256:").ok_or_else(invalid)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    hex::decode(value)
        .map_err(|_| invalid())?
        .try_into()
        .map_err(|_| invalid())
}
fn upload_reply(
    repository: &str,
    id: ResourceId,
    size: u64,
    status: StatusCode,
) -> Result<Response, PlatformError> {
    let mut response = reply(
        status,
        Body::empty(),
        Some(format!("/v2/{repository}/blobs/uploads/{id}")),
        None,
    );
    response.headers_mut().insert(
        "docker-upload-uuid",
        id.to_string().parse().map_err(|_| invalid())?,
    );
    response.headers_mut().insert(
        header::RANGE,
        format!("0-{}", size.saturating_sub(1))
            .parse()
            .map_err(|_| invalid())?,
    );
    Ok(response)
}
fn reply(
    status: StatusCode,
    body: Body,
    location: Option<String>,
    digest: Option<[u8; 32]>,
) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        "docker-distribution-api-version",
        header::HeaderValue::from_static("registry/2.0"),
    );
    if let Some(location) = location.and_then(|v| v.parse().ok()) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    if let Some(digest) = digest.and_then(|v| format!("sha256:{}", hex::encode(v)).parse().ok()) {
        response
            .headers_mut()
            .insert("docker-content-digest", digest);
    }
    response
}
fn unauthorized() -> Response {
    let mut response = failure(StatusCode::UNAUTHORIZED, "UNAUTHORIZED");
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Basic realm=\"Container Registry\""),
    );
    response
}
fn failure(status: StatusCode, code: &str) -> Response {
    (status,axum::Json(serde_json::json!({"errors":[{"code":code,"message":"Container registry request failed"}]}))).into_response()
}
fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Container registry request failed",
    )
}
fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Container registry object was not found",
    )
}

#[cfg(test)]
#[path = "container_registry_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "container_registry_mount_tests.rs"]
mod mount_tests;
