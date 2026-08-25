//! Provider capability canary required by the P0.5 R2 data plane.

use crate::client::S3ArtifactClient;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use rand::Rng as _;
use std::collections::HashMap;

/// Successful R2 provider capability observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct R2PreflightOutcome {
    /// Whether one atomic multi-delete request succeeded.
    pub multi_delete: bool,
    /// Number of distinct arbitrary-key objects verified.
    pub objects: u8,
}

/// Verify required conditional, metadata, range, list, and consistency behavior.
pub async fn preflight_r2(
    client: &S3ArtifactClient,
    platform_id: PlatformId,
    startup_id: StartupId,
) -> Result<R2PreflightOutcome, PlatformError> {
    let mut nonce = [0_u8; 16];
    rand::rng().fill(&mut nonce);
    let root = format!(
        "{}preflight/{platform_id}/{startup_id}/{}/objects/",
        client.r2_prefix(),
        hex::encode(nonce)
    );
    let keys = [
        format!("{root}a +%雪//z"),
        format!("{root}folder/a"),
        format!("{root}folder/sub/b"),
    ];
    let result = run(client, &root, &keys).await;
    cleanup(client, &keys).await;
    result
}

async fn run(
    client: &S3ArtifactClient,
    root: &str,
    keys: &[String; 3],
) -> Result<R2PreflightOutcome, PlatformError> {
    let initial = b"abcdef";
    let mut metadata = HashMap::new();
    metadata.insert("oc-r2-schema".to_owned(), "1".to_owned());
    metadata.insert("oc-r2-custom".to_owned(), "roundtrip".to_owned());
    put(
        client,
        &keys[0],
        initial,
        Some(metadata.clone()),
        None,
        Some("*"),
    )
    .await?;
    for (key, body) in [
        (&keys[1], b"two".as_slice()),
        (&keys[2], b"three".as_slice()),
    ] {
        put(client, key, body, None, None, None).await?;
    }

    let head = client
        .inner()
        .head_object()
        .bucket(client.bucket())
        .key(&keys[0])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if head.content_length() != Some(initial.len() as i64)
        || head
            .metadata()
            .and_then(|values| values.get("oc-r2-schema"))
            .map(String::as_str)
            != Some("1")
        || head
            .metadata()
            .and_then(|values| values.get("oc-r2-custom"))
            .map(String::as_str)
            != Some("roundtrip")
        || head.content_type() != Some("application/octet-stream")
    {
        return Err(integrity("head_metadata"));
    }
    let etag = head
        .e_tag()
        .ok_or_else(|| integrity("head_etag"))?
        .to_owned();

    let range = client
        .inner()
        .get_object()
        .bucket(client.bucket())
        .key(&keys[0])
        .range("bytes=1-3")
        .send()
        .await
        .map_err(|_| unavailable())?;
    if range.content_range() != Some("bytes 1-3/6")
        || range
            .body
            .collect()
            .await
            .map_err(|_| unavailable())?
            .into_bytes()
            .as_ref()
            != b"bcd"
    {
        return Err(integrity("range"));
    }

    let duplicate = client
        .inner()
        .put_object()
        .bucket(client.bucket())
        .key(&keys[0])
        .body(ByteStream::from_static(b"wrong"))
        .content_length(5)
        .if_none_match("*")
        .send()
        .await;
    if !matches!(duplicate.as_ref().err().and_then(status), Some(409 | 412)) {
        return Err(integrity("if_none_match"));
    }
    let wrong_match = client
        .inner()
        .put_object()
        .bucket(client.bucket())
        .key(&keys[0])
        .body(ByteStream::from_static(b"wrong"))
        .content_length(5)
        .if_match("\"wrong\"")
        .send()
        .await;
    if !matches!(wrong_match.as_ref().err().and_then(status), Some(409 | 412)) {
        return Err(integrity("if_match_reject"));
    }
    put(
        client,
        &keys[0],
        b"updated",
        Some(metadata),
        Some(&etag),
        None,
    )
    .await?;
    let updated = client
        .inner()
        .get_object()
        .bucket(client.bucket())
        .key(&keys[0])
        .send()
        .await
        .map_err(|_| unavailable())?
        .body
        .collect()
        .await
        .map_err(|_| unavailable())?
        .into_bytes();
    if updated.as_ref() != b"updated" {
        return Err(integrity("overwrite"));
    }

    let first = client
        .inner()
        .list_objects_v2()
        .bucket(client.bucket())
        .prefix(root)
        .max_keys(1)
        .send()
        .await
        .map_err(|_| unavailable())?;
    let token = first
        .next_continuation_token()
        .ok_or_else(|| integrity("list_token"))?;
    if first.contents().len() != 1 {
        return Err(integrity("list_first"));
    }
    let second = client
        .inner()
        .list_objects_v2()
        .bucket(client.bucket())
        .prefix(root)
        .continuation_token(token)
        .max_keys(2)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if second.contents().len() != 2 {
        return Err(integrity("list_second"));
    }
    let delimited = client
        .inner()
        .list_objects_v2()
        .bucket(client.bucket())
        .prefix(format!("{root}folder/"))
        .delimiter("/")
        .send()
        .await
        .map_err(|_| unavailable())?;
    if delimited.contents().len() != 1
        || delimited.common_prefixes().len() != 1
        || delimited.common_prefixes()[0].prefix() != Some(format!("{root}folder/sub/").as_str())
    {
        return Err(integrity("list_delimiter"));
    }

    let identifiers = keys
        .iter()
        .map(|key| {
            ObjectIdentifier::builder()
                .key(key)
                .build()
                .map_err(|_| integrity("delete_key"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let delete = Delete::builder()
        .set_objects(Some(identifiers))
        .quiet(true)
        .build()
        .map_err(|_| integrity("delete_request"))?;
    let multi_delete = match client
        .inner()
        .delete_objects()
        .bucket(client.bucket())
        .delete(delete)
        .send()
        .await
    {
        Ok(output) if output.errors().is_empty() => true,
        Err(error) if matches!(status(&error), Some(405 | 501)) => {
            cleanup(client, keys).await;
            false
        }
        _ => return Err(unavailable()),
    };
    let empty = client
        .inner()
        .list_objects_v2()
        .bucket(client.bucket())
        .prefix(root)
        .max_keys(1)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !empty.contents().is_empty() {
        return Err(integrity("list_after_delete"));
    }
    Ok(R2PreflightOutcome {
        multi_delete,
        objects: 3,
    })
}

async fn put(
    client: &S3ArtifactClient,
    key: &str,
    body: &[u8],
    metadata: Option<HashMap<String, String>>,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
) -> Result<(), PlatformError> {
    let mut request = client
        .inner()
        .put_object()
        .bucket(client.bucket())
        .key(key)
        .body(ByteStream::from(body.to_vec()))
        .content_length(i64::try_from(body.len()).map_err(|_| integrity("put_length"))?)
        .content_type("application/octet-stream")
        .set_metadata(metadata);
    if let Some(value) = if_match {
        request = request.if_match(value);
    }
    if let Some(value) = if_none_match {
        request = request.if_none_match(value);
    }
    request.send().await.map(|_| ()).map_err(|_| unavailable())
}

async fn cleanup(client: &S3ArtifactClient, keys: &[String]) {
    for key in keys {
        let _ = client
            .inner()
            .delete_object()
            .bucket(client.bucket())
            .key(key)
            .send()
            .await;
    }
}

fn status<E>(error: &SdkError<E, HttpResponse>) -> Option<u16> {
    match error {
        SdkError::ServiceError(error) => Some(error.raw().status().as_u16()),
        SdkError::ResponseError(error) => Some(error.raw().status().as_u16()),
        _ => None,
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 provider capability preflight failed",
    )
}

fn integrity(stage: &'static str) -> PlatformError {
    let message = match stage {
        "head_metadata" => "R2 provider head metadata capability is incompatible",
        "head_etag" => "R2 provider head ETag capability is incompatible",
        "range" => "R2 provider range capability is incompatible",
        "if_none_match" => "R2 provider If-None-Match capability is incompatible",
        "if_match_reject" => "R2 provider If-Match rejection capability is incompatible",
        "overwrite" => "R2 provider conditional overwrite capability is incompatible",
        "list_token" | "list_first" | "list_second" => {
            "R2 provider pagination capability is incompatible"
        }
        "list_delimiter" => "R2 provider delimiter capability is incompatible",
        "delete_key" | "delete_request" => "R2 provider multi-delete request is invalid",
        "list_after_delete" => "R2 provider delete visibility capability is incompatible",
        _ => "R2 provider capability contract is incompatible",
    };
    PlatformError::new(ErrorCode::R2ProviderUnavailable, message)
}
