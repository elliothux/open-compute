//! Backend capability canary required by the R2 data plane.

use crate::backend::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ObjectBackend, ObjectHttpMetadata,
    ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, PutMode, PutOptions, UploadedPart,
};
use bytes::Bytes;
use md5::Digest as _;
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use rand::Rng as _;
use std::collections::BTreeMap;

/// Successful R2 backend capability observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct R2PreflightOutcome {
    /// Whether a bounded multi-object delete succeeded.
    pub multi_delete: bool,
    /// Number of distinct physical objects verified.
    pub objects: u8,
}

/// Verify required conditional, metadata, range, list, and delete behavior.
pub async fn preflight_r2(
    backend: &ObjectBackend,
    platform_id: PlatformId,
    startup_id: StartupId,
) -> Result<R2PreflightOutcome, PlatformError> {
    let mut nonce = [0_u8; 16];
    rand::rng().fill(&mut nonce);
    let root = format!(
        "{}preflight/{platform_id}/{startup_id}/{}/objects/",
        backend.r2_prefix(),
        hex::encode(nonce)
    );
    let keys = [
        ObjectKey::new(format!("{root}a")),
        ObjectKey::new(format!("{root}folder-a")),
        ObjectKey::new(format!("{root}folder-b")),
        ObjectKey::new(format!("{root}multipart")),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|_| integrity("key"))?;
    let result = run(backend, &root, &keys).await;
    let _ = backend.delete_many(&keys).await;
    result
}

async fn run(
    backend: &ObjectBackend,
    root: &str,
    keys: &[ObjectKey],
) -> Result<R2PreflightOutcome, PlatformError> {
    let initial = Bytes::from_static(b"abcdef");
    let metadata = BTreeMap::from([
        ("oc-r2-schema".to_owned(), "1".to_owned()),
        ("oc-r2-custom".to_owned(), "roundtrip".to_owned()),
    ]);
    put(
        backend,
        &keys[0],
        initial.clone(),
        metadata.clone(),
        PutMode::CreateOnly,
    )
    .await?;
    put(
        backend,
        &keys[1],
        Bytes::from_static(b"two"),
        BTreeMap::new(),
        PutMode::Replace,
    )
    .await?;
    put(
        backend,
        &keys[2],
        Bytes::from_static(b"three"),
        BTreeMap::new(),
        PutMode::Replace,
    )
    .await?;

    let head = backend
        .head(&keys[0], HeadOptions::default())
        .await
        .map_err(|_| unavailable())?;
    if head.size != initial.len() as u64
        || head.etag != hex::encode(md5::Md5::digest(&initial))
        || head.user.get("oc-r2-schema").map(String::as_str) != Some("1")
        || head.user.get("oc-r2-custom").map(String::as_str) != Some("roundtrip")
        || head.http.content_type.as_deref() != Some("application/octet-stream")
    {
        return Err(integrity("head_metadata"));
    }

    let range = backend
        .get(
            &keys[0],
            GetOptions {
                range: Some(ObjectRange { start: 1, end: 3 }),
                ..GetOptions::default()
            },
        )
        .await
        .map_err(|_| unavailable())?;
    if range.range != Some(ObjectRange { start: 1, end: 3 })
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

    let duplicate = put(
        backend,
        &keys[0],
        Bytes::from_static(b"wrong"),
        BTreeMap::new(),
        PutMode::CreateOnly,
    )
    .await;
    if duplicate.is_ok() {
        return Err(integrity("if_none_match"));
    }
    let wrong = put(
        backend,
        &keys[0],
        Bytes::from_static(b"wrong"),
        BTreeMap::new(),
        PutMode::IfMatch("wrong".to_owned()),
    )
    .await;
    if wrong.is_ok() {
        return Err(integrity("if_match_reject"));
    }
    put(
        backend,
        &keys[0],
        Bytes::from_static(b"updated"),
        metadata,
        PutMode::IfMatch(head.etag),
    )
    .await?;
    let updated = backend
        .get(&keys[0], GetOptions::default())
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

    let first = backend
        .list(root, 1, None)
        .await
        .map_err(|_| unavailable())?;
    if first.objects.len() != 1 || first.next_cursor.is_none() {
        return Err(integrity("list_first"));
    }
    let second = backend
        .list(root, 2, first.next_cursor.as_deref())
        .await
        .map_err(|_| unavailable())?;
    if second.objects.len() != 2 {
        return Err(integrity("list_second"));
    }
    verify_multipart(backend, &keys[3]).await?;
    let multi_delete = backend.delete_many(keys).await.map_err(|_| unavailable())?;
    if !backend
        .list(root, 1, None)
        .await
        .map_err(|_| unavailable())?
        .objects
        .is_empty()
    {
        return Err(integrity("list_after_delete"));
    }
    Ok(R2PreflightOutcome {
        multi_delete,
        objects: 4,
    })
}

async fn verify_multipart(backend: &ObjectBackend, key: &ObjectKey) -> Result<(), PlatformError> {
    let customer = CustomerKey::new([0x5a; 32]);
    let upload_id = backend
        .create_multipart(key, ObjectMetadata::default(), Some(customer.clone()))
        .await
        .map_err(|_| unavailable())?;
    let result = async {
        let mut parts = Vec::new();
        for (part_number, body) in [(1, b"part-a".as_slice()), (2, b"part-b".as_slice())] {
            let part = backend
                .upload_part(
                    key,
                    &upload_id,
                    part_number,
                    ObjectSource::Bytes(Bytes::copy_from_slice(body)),
                    Some(customer.clone()),
                )
                .await
                .map_err(|_| unavailable())?;
            if part.etag != hex::encode(md5::Md5::digest(body)) {
                return Err(integrity("multipart_part_etag"));
            }
            parts.push(part);
        }
        let completed = backend
            .complete_multipart(key, &upload_id, &parts, Some(customer.clone()))
            .await
            .map_err(|_| unavailable())?;
        let expected_etag = multipart_etag(&parts)?;
        if completed.size != 12 || completed.etag != expected_etag {
            return Err(integrity("multipart_complete"));
        }
        let body = backend
            .get(
                key,
                GetOptions {
                    customer_key: Some(customer.clone()),
                    ..GetOptions::default()
                },
            )
            .await
            .map_err(|_| unavailable())?
            .body
            .collect()
            .await
            .map_err(|_| unavailable())?
            .into_bytes();
        if body.as_ref() != b"part-apart-b" {
            return Err(integrity("multipart_body"));
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = backend.abort_multipart(key, &upload_id).await;
    }
    result
}

fn multipart_etag(parts: &[UploadedPart]) -> Result<String, PlatformError> {
    let mut binary = Vec::with_capacity(parts.len().saturating_mul(16));
    for part in parts {
        let digest = hex::decode(&part.etag).map_err(|_| integrity("multipart_part_etag"))?;
        if digest.len() != 16 {
            return Err(integrity("multipart_part_etag"));
        }
        binary.extend(digest);
    }
    Ok(format!(
        "{}-{}",
        hex::encode(md5::Md5::digest(binary)),
        parts.len()
    ))
}

async fn put(
    backend: &ObjectBackend,
    key: &ObjectKey,
    body: Bytes,
    user: BTreeMap<String, String>,
    mode: PutMode,
) -> Result<(), PlatformError> {
    backend
        .put(
            key,
            ObjectSource::Bytes(body),
            PutOptions {
                mode,
                metadata: ObjectMetadata {
                    user,
                    http: ObjectHttpMetadata {
                        content_type: Some("application/octet-stream".to_owned()),
                        ..ObjectHttpMetadata::default()
                    },
                    ..ObjectMetadata::default()
                },
                customer_key: None,
            },
        )
        .await
        .map(|_| ())
        .map_err(|failure| match failure {
            BackendError::PreconditionFailed => integrity("precondition"),
            _ => unavailable(),
        })
}

const fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 backend capability preflight failed",
    )
}

fn integrity(stage: &'static str) -> PlatformError {
    let message = match stage {
        "head_metadata" => "R2 backend head metadata or ETag capability is incompatible",
        "range" => "R2 backend range capability is incompatible",
        "if_none_match" => "R2 backend If-None-Match capability is incompatible",
        "if_match_reject" => "R2 backend If-Match rejection capability is incompatible",
        "overwrite" => "R2 backend conditional overwrite capability is incompatible",
        "list_first" | "list_second" => "R2 backend pagination capability is incompatible",
        "list_after_delete" => "R2 backend delete visibility capability is incompatible",
        "multipart_part_etag" | "multipart_complete" | "multipart_body" => {
            "R2 backend multipart or SSE-C capability is incompatible"
        }
        _ => "R2 backend capability contract is incompatible",
    };
    PlatformError::new(ErrorCode::R2ProviderUnavailable, message)
}
