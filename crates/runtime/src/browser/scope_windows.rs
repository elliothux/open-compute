//! Page-owned window identifiers used by the fixed Playwright viewport initializer.

use super::{denied, require_fields};
use open_compute_core::PlatformError;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(super) struct Window {
    pub(super) engine: u64,
    pub(super) target: String,
}

pub(super) fn command(
    method: &str,
    fields: &mut Map<String, Value>,
    windows: &BTreeMap<u64, Window>,
    targets: &BTreeMap<String, String>,
) -> Result<(), PlatformError> {
    if method == "Browser.getWindowForTarget" {
        require_fields(fields, &["targetId"])?;
        if let Some(target) = fields.get_mut("targetId") {
            *target = targets
                .get(target.as_str().ok_or_else(denied)?)
                .ok_or_else(denied)?
                .clone()
                .into();
        }
    } else {
        require_fields(
            fields,
            if method == "Browser.setWindowBounds" {
                &["windowId", "bounds"]
            } else {
                &["windowId"]
            },
        )?;
        let id = fields
            .get("windowId")
            .and_then(Value::as_u64)
            .ok_or_else(denied)?;
        let window = windows.get(&id).ok_or_else(denied)?;
        if !targets.contains_key(&window.target) {
            return Err(denied());
        }
        fields.insert("windowId".into(), window.engine.into());
    }
    Ok(())
}

pub(super) fn reply(
    original: &Value,
    result: &mut Map<String, Value>,
    windows: &mut BTreeMap<u64, Window>,
) -> Result<(), PlatformError> {
    let engine = result
        .get("windowId")
        .and_then(Value::as_u64)
        .ok_or_else(denied)?;
    let target = original
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(denied)?;
    let public = if let Some((public, _)) = windows
        .iter()
        .find(|(_, window)| window.engine == engine && window.target == target)
    {
        *public
    } else {
        if windows.len() >= 4096 {
            return Err(denied());
        }
        let public = windows.len() as u64 + 1;
        windows.insert(
            public,
            Window {
                engine,
                target: target.to_owned(),
            },
        );
        public
    };
    result.insert("windowId".into(), public.into());
    Ok(())
}
