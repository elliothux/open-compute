//! Strict fixed-Wrangler query decoders for Worker routes.

use crate::cloudflare_v4::V4Error;
use std::collections::BTreeSet;

pub(super) struct UploadQuery {
    pub(super) strict_inheritance: bool,
}

pub(super) fn upload(query: Option<&str>, put_script: bool) -> Result<UploadQuery, V4Error> {
    let mut strict = false;
    let mut seen = BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if !seen.insert(key.clone()) {
            return Err(V4Error::InvalidRequest);
        }
        match (key.as_ref(), value.as_ref()) {
            ("bindings_inherit", "strict") => strict = true,
            // Fixed Wrangler uses this response-projection flag for every
            // Script PUT, including uploads that contain Worker modules.
            ("excludeScript", "true") if put_script => {}
            _ => return Err(V4Error::InvalidRequest),
        }
    }
    Ok(UploadQuery {
        strict_inheritance: strict,
    })
}

pub(super) fn deployment_force(query: Option<&str>) -> Result<bool, V4Error> {
    let mut force = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if force.is_some() || key != "force" || !matches!(value.as_ref(), "true" | "false") {
            return Err(V4Error::InvalidRequest);
        }
        force = Some(value == "true");
    }
    Ok(force.unwrap_or(false))
}

pub(super) struct VersionListQuery {
    pub(super) deployable: bool,
    pub(super) page: usize,
    pub(super) per_page: usize,
}

pub(super) fn version_list(query: Option<&str>) -> Result<VersionListQuery, V4Error> {
    let mut result = VersionListQuery {
        deployable: false,
        page: 1,
        per_page: 100,
    };
    let mut seen = BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if !seen.insert(key.clone()) {
            return Err(V4Error::InvalidRequest);
        }
        match key.as_ref() {
            "deployable" if matches!(value.as_ref(), "true" | "false") => {
                result.deployable = value == "true";
            }
            "page" => {
                result.page = value.parse().map_err(|_| V4Error::InvalidRequest)?;
                if result.page == 0 {
                    return Err(V4Error::InvalidRequest);
                }
            }
            "per_page" => {
                result.per_page = value.parse().map_err(|_| V4Error::InvalidRequest)?;
                if result.per_page == 0 || result.per_page > 1000 {
                    return Err(V4Error::InvalidRequest);
                }
            }
            _ => return Err(V4Error::InvalidRequest),
        }
    }
    Ok(result)
}
