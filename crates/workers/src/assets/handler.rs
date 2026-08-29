//! Pure static-asset routing and response planning shared by public and binding fetches.

use super::{AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, HtmlHandling, NotFoundHandling};
use open_compute_core::PlatformError;
use std::collections::{BTreeMap, BTreeSet};

/// HTTP request inputs relevant to immutable static-asset routing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetRequest<'a> {
    /// Uppercase request method.
    pub method: &'a str,
    /// Raw URI path, retaining percent escapes.
    pub path: &'a str,
    /// Raw query without `?`.
    pub query: Option<&'a str>,
    /// Canonical request hostname, without a port.
    pub host: &'a str,
    /// `Sec-Fetch-Mode` value when present.
    pub sec_fetch_mode: Option<&'a str>,
    /// `If-None-Match` value when present.
    pub if_none_match: Option<&'a str>,
    /// Whether the request carries `Authorization`.
    pub has_authorization: bool,
    /// Whether the request carries `Range`; the Day1 baseline returns a full response.
    pub has_range: bool,
}

/// Fully deterministic asset response plan before authorized bytes are opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetResponsePlan {
    /// HTTP status.
    pub status: u16,
    /// Manifest entry whose bytes form the response body.
    pub entry: Option<AssetEntryV1>,
    /// Canonical response fields after `_headers` processing.
    pub headers: BTreeMap<String, String>,
    /// Whether request semantics suppress the selected representation body.
    pub head: bool,
}

/// Apply redirects, HTML handling, missing-page behavior, `ETags`, and custom headers.
pub fn plan_asset_response(
    manifest: &AssetManifestV1,
    routing: &AssetRoutingConfigV1,
    request: AssetRequest<'_>,
) -> Result<AssetResponsePlan, PlatformError> {
    manifest.validate()?;
    routing.validate()?;
    if !matches!(request.method, "GET" | "HEAD") {
        return Ok(empty_plan(405, request.method == "HEAD"));
    }
    let mut path = request.path.to_owned();
    for redirect in &routing.redirects {
        let Some(captures) = match_rule(&redirect.from, request.host, &path) else {
            continue;
        };
        let destination = substitute(&redirect.to, &captures);
        if redirect.status == 200 {
            path = split_path(&destination).0.to_owned();
            break;
        }
        let location = redirect_location(&destination, request.query);
        let mut plan = empty_plan(redirect.status, request.method == "HEAD");
        plan.headers.insert("location".to_owned(), location);
        return Ok(plan);
    }
    let selection = select_asset(manifest, routing.html_handling, &path);
    let (status, entry) = match selection {
        Selection::Entry(entry) => (200, Some(entry.clone())),
        Selection::Redirect(location) => {
            let mut plan = empty_plan(307, request.method == "HEAD");
            plan.headers.insert(
                "location".to_owned(),
                redirect_location(&location, request.query),
            );
            return Ok(plan);
        }
        Selection::Missing => {
            let (status, entry) = missing_entry(manifest, routing.not_found_handling, &path);
            (status, entry.cloned())
        }
    };
    let Some(entry) = entry else {
        return Ok(empty_plan(status, request.method == "HEAD"));
    };
    let etag = format!("\"oc-{}\"", entry.sha256);
    let not_modified = if_none_match(request.if_none_match, &etag);
    let mut headers = BTreeMap::from([
        ("content-type".to_owned(), entry.content_type.clone()),
        ("etag".to_owned(), etag),
    ]);
    if !request.has_authorization && !request.has_range {
        headers.insert(
            "cache-control".to_owned(),
            "public, max-age=0, must-revalidate".to_owned(),
        );
    }
    if !not_modified {
        headers.insert("content-length".to_owned(), entry.size.to_string());
    }
    apply_headers(&mut headers, routing, request.host, &path);
    Ok(AssetResponsePlan {
        status: if not_modified { 304 } else { status },
        entry: if not_modified { None } else { Some(entry) },
        headers,
        head: request.method == "HEAD",
    })
}

enum Selection<'a> {
    Entry(&'a AssetEntryV1),
    Redirect(String),
    Missing,
}

fn select_asset<'a>(
    manifest: &'a AssetManifestV1,
    handling: HtmlHandling,
    path: &str,
) -> Selection<'a> {
    if handling == HtmlHandling::None {
        return find(manifest, path).map_or(Selection::Missing, Selection::Entry);
    }
    if handling == HtmlHandling::AutoTrailingSlash {
        let alias = path
            .strip_suffix("/index.html")
            .or_else(|| path.strip_suffix("/index"))
            .or_else(|| path.strip_suffix(".html"));
        if let Some(alias) = alias {
            let alias = if alias.is_empty() { "/" } else { alias };
            let file_candidate = format!("{alias}.html");
            let index_candidate = if alias == "/" {
                "/index.html".to_owned()
            } else {
                format!("{alias}/index.html")
            };
            if find(manifest, &file_candidate).is_some()
                || find(manifest, &index_candidate).is_some()
            {
                return Selection::Redirect(alias.to_owned());
            }
        }
    }
    let trimmed = path.trim_end_matches('/');
    let root = if trimmed.is_empty() { "/" } else { trimmed };
    let without_html = root.strip_suffix(".html").unwrap_or(root);
    let without_index = without_html.strip_suffix("/index").unwrap_or(without_html);
    let canonical_stem = if without_index.is_empty() {
        "/"
    } else {
        without_index
    };
    let file_candidate = format!("{canonical_stem}.html");
    let index_candidate = if canonical_stem == "/" {
        "/index.html".to_owned()
    } else {
        format!("{canonical_stem}/index.html")
    };
    let file = find(manifest, &file_candidate);
    let index = find(manifest, &index_candidate);
    let (entry, canonical) = match handling {
        HtmlHandling::AutoTrailingSlash => match (file, index) {
            (Some(entry), _) => (Some(entry), canonical_stem.to_owned()),
            (None, Some(entry)) => (Some(entry), trailing(canonical_stem)),
            (None, None) => (find(manifest, path), path.to_owned()),
        },
        HtmlHandling::ForceTrailingSlash => match file.or(index) {
            Some(entry) => (Some(entry), trailing(canonical_stem)),
            None => (find(manifest, path), path.to_owned()),
        },
        HtmlHandling::DropTrailingSlash => match file.or(index) {
            Some(entry) => (Some(entry), canonical_stem.to_owned()),
            None => (find(manifest, path), path.to_owned()),
        },
        HtmlHandling::None => return Selection::Missing,
    };
    match entry {
        Some(_) if path != canonical => Selection::Redirect(canonical),
        Some(entry) => Selection::Entry(entry),
        None => Selection::Missing,
    }
}

fn missing_entry<'a>(
    manifest: &'a AssetManifestV1,
    handling: NotFoundHandling,
    request_path: &str,
) -> (u16, Option<&'a AssetEntryV1>) {
    match handling {
        NotFoundHandling::None => (404, None),
        NotFoundHandling::SinglePageApplication => (200, find(manifest, "/index.html")),
        NotFoundHandling::Page404 => {
            let mut parent = request_path.trim_end_matches('/').to_owned();
            loop {
                let slash = parent.rfind('/').unwrap_or(0);
                parent.truncate(slash);
                let candidate = if parent.is_empty() {
                    "/404.html".to_owned()
                } else {
                    format!("{parent}/404.html")
                };
                if let Some(entry) = find(manifest, &candidate) {
                    return (404, Some(entry));
                }
                if parent.is_empty() {
                    return (404, None);
                }
            }
        }
    }
}

fn find<'a>(manifest: &'a AssetManifestV1, path: &str) -> Option<&'a AssetEntryV1> {
    manifest
        .entries
        .binary_search_by(|entry| entry.path.as_str().cmp(path))
        .ok()
        .map(|index| &manifest.entries[index])
}

fn empty_plan(status: u16, head: bool) -> AssetResponsePlan {
    AssetResponsePlan {
        status,
        entry: None,
        headers: BTreeMap::new(),
        head,
    }
}

fn trailing(path: &str) -> String {
    if path == "/" {
        "/".to_owned()
    } else {
        format!("{path}/")
    }
}

fn split_path(value: &str) -> (&str, Option<&str>) {
    value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)))
}

fn redirect_location(destination: &str, query: Option<&str>) -> String {
    if destination.contains('?') || query.is_none() {
        destination.to_owned()
    } else {
        format!("{destination}?{}", query.unwrap_or_default())
    }
}

fn if_none_match(value: Option<&str>, etag: &str) -> bool {
    value.is_some_and(|value| {
        value.split(',').map(str::trim).any(|candidate| {
            candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
        })
    })
}

fn apply_headers(
    headers: &mut BTreeMap<String, String>,
    routing: &AssetRoutingConfigV1,
    host: &str,
    path: &str,
) {
    let mut custom = BTreeSet::new();
    for rule in &routing.headers {
        let Some(captures) = match_rule(&rule.pattern, host, path) else {
            continue;
        };
        for operation in &rule.operations {
            match &operation.value {
                None => {
                    headers.remove(&operation.name);
                    custom.insert(operation.name.clone());
                }
                Some(value) => {
                    let value = substitute(value, &captures);
                    if custom.insert(operation.name.clone()) {
                        headers.insert(operation.name.clone(), value);
                    } else {
                        headers
                            .entry(operation.name.clone())
                            .and_modify(|current| {
                                current.push_str(", ");
                                current.push_str(&value);
                            })
                            .or_insert(value);
                    }
                }
            }
        }
    }
}

fn match_rule(pattern: &str, host: &str, path: &str) -> Option<BTreeMap<String, String>> {
    let target = if let Some(pattern) = pattern.strip_prefix("https://") {
        let full = format!("{host}{path}");
        return match_text(pattern, &full, 0, 0, &mut BTreeMap::new());
    } else {
        path
    };
    match_text(pattern, target, 0, 0, &mut BTreeMap::new())
}

fn match_text(
    pattern: &str,
    target: &str,
    pattern_at: usize,
    target_at: usize,
    captures: &mut BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    if pattern_at == pattern.len() {
        return (target_at == target.len()).then(|| captures.clone());
    }
    let next = pattern.as_bytes()[pattern_at];
    if next == b'*' {
        for end in (target_at..=target.len()).rev() {
            if !target.is_char_boundary(end) {
                continue;
            }
            captures.insert("splat".to_owned(), target[target_at..end].to_owned());
            if let Some(found) = match_text(pattern, target, pattern_at + 1, end, captures) {
                return Some(found);
            }
        }
        captures.remove("splat");
        return None;
    }
    if next == b':' {
        let name_end = pattern[pattern_at + 1..]
            .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .map_or(pattern.len(), |offset| pattern_at + 1 + offset);
        let name = &pattern[pattern_at + 1..name_end];
        let host_placeholder = !pattern[..pattern_at].contains('/');
        let delimiter = if host_placeholder { b'.' } else { b'/' };
        let end = target.as_bytes()[target_at..]
            .iter()
            .position(|byte| *byte == delimiter)
            .map_or(target.len(), |offset| target_at + offset);
        if end == target_at || !target.is_char_boundary(end) {
            return None;
        }
        captures.insert(name.to_owned(), target[target_at..end].to_owned());
        let found = match_text(pattern, target, name_end, end, captures);
        captures.remove(name);
        return found;
    }
    if target.as_bytes().get(target_at).copied() == Some(next) {
        match_text(pattern, target, pattern_at + 1, target_at + 1, captures)
    } else {
        None
    }
}

fn substitute(value: &str, captures: &BTreeMap<String, String>) -> String {
    captures
        .iter()
        .fold(value.to_owned(), |current, (name, capture)| {
            current.replace(&format!(":{name}"), capture)
        })
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
