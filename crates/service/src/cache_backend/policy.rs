//! Canonical cache header policy and conditional/range response planning.

use super::{add_seconds, protocol, put_rejected};
use axum::http::{HeaderMap, HeaderValue, header};
use open_compute_core::PlatformError;
use open_compute_storage::{CacheHeader, CacheMethod, CacheStoredResponse, CacheSurface};
use std::collections::BTreeMap;

pub(super) fn canonical_header_map(
    values: Vec<(String, String)>,
) -> Result<BTreeMap<String, String>, PlatformError> {
    let mut headers = BTreeMap::new();
    for (name, value) in values {
        let name = name.to_ascii_lowercase();
        if name.is_empty() || name.len() > 128 || value.contains(['\r', '\n', '\0']) {
            return Err(protocol());
        }
        headers
            .entry(name)
            .and_modify(|prior: &mut String| {
                prior.push_str(", ");
                prior.push_str(&value);
            })
            .or_insert(value);
    }
    Ok(headers)
}

pub(super) fn canonical_headers(
    values: Vec<(String, String)>,
) -> Result<Vec<CacheHeader>, PlatformError> {
    let map = canonical_header_map(values)?;
    Ok(map
        .into_iter()
        .map(|(name, value)| CacheHeader { name, value })
        .collect())
}

pub(super) fn comma_values(
    headers: &[CacheHeader],
    name: &str,
) -> Result<Vec<String>, PlatformError> {
    let Some(value) = headers
        .iter()
        .find(|header| header.name == name)
        .map(|header| &header.value)
    else {
        return Ok(Vec::new());
    };
    let mut values = value
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if values.iter().any(String::is_empty) {
        return Err(protocol());
    }
    values.sort();
    values.dedup();
    Ok(values)
}

pub(super) fn cache_deadlines(
    headers: &[CacheHeader],
    now: i64,
    surface: CacheSurface,
    status: u16,
) -> Result<Option<(i64, i64, i64)>, PlatformError> {
    let control = [
        "cloudflare-cdn-cache-control",
        "cdn-cache-control",
        "cache-control",
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.value.as_str())
    })
    .unwrap_or("");
    let mut max_age = None;
    let mut swr = 0_u64;
    let mut sie = 0_u64;
    for directive in control.split(',').map(str::trim) {
        let name = directive
            .split_once('=')
            .map_or(directive, |(name, _)| name)
            .trim()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "no-store" | "no-cache" | "private") {
            return Err(put_rejected());
        }
        let Some((name, value)) = directive.split_once('=') else {
            continue;
        };
        let parse_seconds = || {
            value
                .trim()
                .trim_matches('"')
                .parse::<u64>()
                .map_err(|_| put_rejected())
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "s-maxage" => max_age = Some(parse_seconds()?),
            "max-age" if max_age.is_none() => max_age = Some(parse_seconds()?),
            "stale-while-revalidate" if surface == CacheSurface::Automatic => {
                swr = parse_seconds()?;
            }
            "stale-if-error" if surface == CacheSurface::Automatic => {
                sie = parse_seconds()?;
            }
            _ => continue,
        }
    }
    let max_age = match max_age {
        Some(value) => value,
        None if surface == CacheSurface::Automatic => return Err(put_rejected()),
        None => match default_cache_api_ttl(status) {
            Some(value) => value,
            None => return Ok(None),
        },
    };
    let fresh = add_seconds(now, max_age)?;
    Ok(Some((
        fresh,
        add_seconds(fresh, swr)?,
        add_seconds(fresh, sie)?,
    )))
}

fn default_cache_api_ttl(status: u16) -> Option<u64> {
    match status {
        200 | 203 | 204 => Some(2 * 60 * 60),
        300 | 301 => Some(20 * 60),
        404 | 410 => Some(3 * 60),
        405 | 414 | 501 => Some(60),
        _ => None,
    }
}

pub(super) fn has_forbidden_cache_directive(value: &str) -> bool {
    value.split(',').any(|directive| {
        let name = directive
            .split_once('=')
            .map_or(directive, |(name, _)| name)
            .trim();
        matches!(
            name.to_ascii_lowercase().as_str(),
            "no-store" | "no-cache" | "private"
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CachedResponsePlan {
    Full,
    Empty { status: u16 },
    Range { start: u64, length: u64 },
}

pub(super) fn cached_response_plan(
    stored: &CacheStoredResponse,
    request: &BTreeMap<String, String>,
    method: CacheMethod,
    response_headers: &mut HeaderMap,
) -> Result<CachedResponsePlan, PlatformError> {
    let etag = response_headers
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok());
    if request
        .get("if-none-match")
        .is_some_and(|condition| etag.is_some_and(|etag| etag_matches(condition, etag)))
    {
        return Ok(CachedResponsePlan::Empty { status: 304 });
    }
    if !request.contains_key("if-none-match")
        && let (Some(condition), Some(modified)) = (
            request.get("if-modified-since"),
            response_headers
                .get(header::LAST_MODIFIED)
                .and_then(|value| value.to_str().ok()),
        )
        && let (Ok(condition), Ok(modified)) = (
            httpdate::parse_http_date(condition),
            httpdate::parse_http_date(modified),
        )
        && modified <= condition
    {
        return Ok(CachedResponsePlan::Empty { status: 304 });
    }
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&stored.body.size.to_string()).map_err(|_| protocol())?,
    );
    if method != CacheMethod::Get || stored.status != 200 {
        return Ok(CachedResponsePlan::Full);
    }
    let Some(range) = request.get("range") else {
        return Ok(CachedResponsePlan::Full);
    };
    let Some(specification) = range.strip_prefix("bytes=") else {
        return range_unsatisfied(stored.body.size, response_headers);
    };
    if specification.contains(',') {
        return range_unsatisfied(stored.body.size, response_headers);
    }
    let Some((start, end)) = specification.split_once('-') else {
        return range_unsatisfied(stored.body.size, response_headers);
    };
    let selected = if start.is_empty() {
        let suffix = end.parse::<u64>().ok().filter(|value| *value > 0);
        suffix.map(|suffix| {
            let length = suffix.min(stored.body.size);
            (stored.body.size.saturating_sub(length), length)
        })
    } else {
        let start = start.parse::<u64>().ok();
        let end = if end.is_empty() {
            Some(stored.body.size.saturating_sub(1))
        } else {
            end.parse::<u64>().ok()
        };
        start.zip(end).and_then(|(start, end)| {
            if start >= stored.body.size || end < start {
                None
            } else {
                let end = end.min(stored.body.size.saturating_sub(1));
                Some((start, end.saturating_sub(start).saturating_add(1)))
            }
        })
    };
    let Some((start, length)) = selected else {
        return range_unsatisfied(stored.body.size, response_headers);
    };
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!(
            "bytes {start}-{}/{size}",
            start.saturating_add(length).saturating_sub(1),
            size = stored.body.size,
        ))
        .map_err(|_| protocol())?,
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).map_err(|_| protocol())?,
    );
    Ok(CachedResponsePlan::Range { start, length })
}

fn range_unsatisfied(
    size: u64,
    headers: &mut HeaderMap,
) -> Result<CachedResponsePlan, PlatformError> {
    headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{size}")).map_err(|_| protocol())?,
    );
    Ok(CachedResponsePlan::Empty { status: 416 })
}

fn etag_matches(condition: &str, etag: &str) -> bool {
    let etag = etag.strip_prefix("W/").unwrap_or(etag).trim();
    condition.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate).trim() == etag
    })
}
