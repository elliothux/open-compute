//! Object-metadata codec used by the typed R2 store.

use crate::ObjectMetadata;
use crate::r2_model::{
    R2_MAX_CUSTOM_METADATA_JSON_BYTES, R2Checksums, R2HttpMetadata, R2ObjectMetadata, R2Range,
    R2StorageClass, invalid_options,
};
use base64::Engine as _;
use open_compute_core::{ErrorCode, PlatformError};
use std::collections::BTreeMap;

pub(crate) const META_SCHEMA: &str = "oc-r2-schema";
pub(crate) const META_VERSION: &str = "oc-r2-version";
pub(crate) const META_CUSTOM: &str = "oc-r2-custom";
pub(crate) const META_HTTP_FIELDS: &str = "oc-r2-http-fields";
pub(crate) const META_MD5: &str = "oc-r2-md5";
pub(crate) const META_SHA1: &str = "oc-r2-sha1";
pub(crate) const META_SHA256: &str = "oc-r2-sha256";
pub(crate) const META_SHA384: &str = "oc-r2-sha384";
pub(crate) const META_SHA512: &str = "oc-r2-sha512";
pub(crate) const META_STORAGE: &str = "oc-r2-storage";
pub(crate) const META_SSEC_MD5: &str = "oc-r2-ssec-md5";
pub(crate) const OBJECTS_SUFFIX: &str = "objects/";

pub(crate) fn canonical_custom_metadata(
    metadata: &BTreeMap<String, String>,
) -> Result<Vec<u8>, PlatformError> {
    let bytes = serde_json::to_vec(metadata).map_err(|_| invalid_options())?;
    if bytes.len() > R2_MAX_CUSTOM_METADATA_JSON_BYTES {
        return Err(PlatformError::new(
            ErrorCode::R2MetadataTooLarge,
            "R2 custom metadata exceeds the canonical JSON budget",
        ));
    }
    Ok(bytes)
}

pub(crate) fn encode_custom_metadata(
    metadata: &BTreeMap<String, String>,
) -> Result<String, PlatformError> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(canonical_custom_metadata(metadata)?))
}

pub(crate) fn decode_metadata(
    key: &str,
    object: &ObjectMetadata,
    range: Option<R2Range>,
) -> Result<R2ObjectMetadata, PlatformError> {
    let metadata = &object.user;
    if metadata.get(META_SCHEMA).map(String::as_str) != Some("1") {
        return Err(integrity_error());
    }
    let version = metadata.get(META_VERSION).ok_or_else(integrity_error)?;
    let parsed = uuid::Uuid::parse_str(version).map_err(|_| integrity_error())?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != *version {
        return Err(integrity_error());
    }
    let custom = metadata.get(META_CUSTOM).ok_or_else(integrity_error)?;
    let custom_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(custom)
        .map_err(|_| integrity_error())?;
    if custom_bytes.len() > R2_MAX_CUSTOM_METADATA_JSON_BYTES {
        return Err(integrity_error());
    }
    let custom_metadata: BTreeMap<String, String> =
        serde_json::from_slice(&custom_bytes).map_err(|_| integrity_error())?;
    if canonical_custom_metadata(&custom_metadata).map_err(|_| integrity_error())? != custom_bytes {
        return Err(integrity_error());
    }
    let raw_fields = metadata.get(META_HTTP_FIELDS).ok_or_else(integrity_error)?;
    let fields: u8 = raw_fields.parse().map_err(|_| integrity_error())?;
    if fields > 63 || fields.to_string() != *raw_fields {
        return Err(integrity_error());
    }
    let checksums = R2Checksums {
        md5: hex_checksum(metadata.get(META_MD5), 32)?,
        sha1: hex_checksum(metadata.get(META_SHA1), 40)?,
        sha256: hex_checksum(metadata.get(META_SHA256), 64)?,
        sha384: hex_checksum(metadata.get(META_SHA384), 96)?,
        sha512: hex_checksum(metadata.get(META_SHA512), 128)?,
    };
    let storage_class = metadata
        .get(META_STORAGE)
        .map_or("Standard", String::as_str);
    R2StorageClass::parse(storage_class).map_err(|_| integrity_error())?;
    let ssec_key_md5 = match metadata.get(META_SSEC_MD5) {
        None => None,
        Some(value)
            if value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            if hex::decode(value).is_err() {
                return Err(integrity_error());
            }
            Some(value.clone())
        }
        Some(_) => return Err(integrity_error()),
    };
    let etag = unquote_etag(&object.etag)?;
    let http_etag = quote_etag(&etag)?;
    Ok(R2ObjectMetadata {
        key: key.to_owned(),
        version: version.to_owned(),
        size: object.size,
        etag,
        http_etag,
        uploaded: object.last_modified_ms,
        http_metadata: Some(R2HttpMetadata {
            content_type: declared_http_field(fields, 1, &object.http.content_type)?,
            content_language: declared_http_field(fields, 2, &object.http.content_language)?,
            content_disposition: declared_http_field(fields, 4, &object.http.content_disposition)?,
            content_encoding: declared_http_field(fields, 8, &object.http.content_encoding)?,
            cache_control: declared_http_field(fields, 16, &object.http.cache_control)?,
            cache_expiry: declared_http_field(fields, 32, &object.http.cache_expiry)?,
        }),
        custom_metadata: Some(custom_metadata),
        range,
        checksums,
        storage_class: storage_class.to_owned(),
        ssec_key_md5,
    })
}

// S3 providers may synthesize headers such as Content-Type. Only tenant-declared
// fields belong to the R2 object; a missing declared field is still corruption.
fn declared_http_field<T: Clone>(
    fields: u8,
    bit: u8,
    value: &Option<T>,
) -> Result<Option<T>, PlatformError> {
    if fields & bit == 0 {
        Ok(None)
    } else {
        value.clone().map(Some).ok_or_else(integrity_error)
    }
}

fn hex_checksum(value: Option<&String>, len: usize) -> Result<Option<String>, PlatformError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() != len
        || hex::decode(value).is_err()
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(integrity_error());
    }
    Ok(Some(value.clone()))
}

pub(crate) fn unquote_etag(value: &str) -> Result<String, PlatformError> {
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"')
    {
        return Err(integrity_error());
    }
    Ok(value.to_owned())
}

pub(crate) fn quote_etag(value: &str) -> Result<String, PlatformError> {
    let value = unquote_etag(value)?;
    Ok(format!("\"{value}\""))
}

pub(crate) fn integrity_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ObjectMetadataInvalid,
        "R2 object metadata failed integrity validation",
    )
}
