//! Keep provider body hashing off the application's shared Tokio scheduler.

use super::*;
use std::collections::BTreeMap;

pub(super) async fn hash_part(
    body: Vec<u8>,
    opaque: bool,
) -> Result<(Vec<u8>, String), std::io::Error> {
    tokio::task::spawn_blocking(move || {
        let etag = if opaque {
            format!("opaque-{}", hex::encode(Sha256::digest(&body)))
        } else {
            hex::encode(md5::Md5::digest(&body))
        };
        (body, etag)
    })
    .await
    .map_err(std::io::Error::other)
}

pub(super) async fn hash_put(
    body: Vec<u8>,
    supplied_sha256: Option<String>,
) -> Result<(Vec<u8>, String, String), std::io::Error> {
    tokio::task::spawn_blocking(move || {
        let sha256 = supplied_sha256.unwrap_or_else(|| hex::encode(Sha256::digest(&body)));
        let etag = hex::encode(md5::Md5::digest(&body));
        (body, sha256, etag)
    })
    .await
    .map_err(std::io::Error::other)
}

pub(super) async fn assemble_parts(
    parts: BTreeMap<i32, (String, Vec<u8>)>,
) -> Result<(Vec<u8>, String, String), std::io::Error> {
    tokio::task::spawn_blocking(move || {
        let mut assembled = Vec::new();
        let mut part_digests = Vec::new();
        let mut md5_parts = true;
        let part_count = parts.len();
        for (_number, (etag, part)) in parts {
            match hex::decode(etag) {
                Ok(digest) if digest.len() == 16 => part_digests.extend_from_slice(&digest),
                Ok(_) | Err(_) => md5_parts = false,
            }
            assembled.extend_from_slice(&part);
        }
        let sha256 = hex::encode(Sha256::digest(&assembled));
        let etag = if md5_parts {
            format!(
                "{}-{part_count}",
                hex::encode(md5::Md5::digest(part_digests))
            )
        } else {
            format!(
                "opaque-{}-{part_count}",
                hex::encode(Sha256::digest(&assembled))
            )
        };
        (assembled, sha256, etag)
    })
    .await
    .map_err(std::io::Error::other)
}
