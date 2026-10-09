//! CDP frame identities share target aliases, while target admission remains context-proven.

use super::{Namespace, alias, denied};
use open_compute_core::PlatformError;
use serde_json::{Map, Value};

pub(super) fn target(namespace: &mut Namespace, engine: &str) -> String {
    if let Some((public, _)) = namespace
        .frames
        .iter()
        .find(|(_, raw)| raw.as_str() == engine)
    {
        let public = public.clone();
        namespace.targets.insert(public.clone(), engine.to_owned());
        return public;
    }
    alias(&mut namespace.targets, engine)
}

fn frame(namespace: &mut Namespace, engine: &str) -> String {
    if let Some((public, _)) = namespace
        .targets
        .iter()
        .find(|(_, raw)| raw.as_str() == engine)
    {
        let public = public.clone();
        namespace.frames.insert(public.clone(), engine.to_owned());
        return public;
    }
    alias(&mut namespace.frames, engine)
}

pub(super) fn command(
    method: &str,
    fields: &mut Map<String, Value>,
    namespace: &Namespace,
) -> Result<(), PlatformError> {
    if matches!(
        method,
        "Page.navigate"
            | "Page.createIsolatedWorld"
            | "Page.setDocumentContent"
            | "DOM.getFrameOwner"
    ) && let Some(value) = fields.get_mut("frameId")
    {
        *value = namespace
            .frames
            .get(value.as_str().ok_or_else(denied)?)
            .ok_or_else(denied)?
            .clone()
            .into();
    }
    Ok(())
}

fn field(value: &mut Value, key: &str, namespace: &mut Namespace) -> Result<(), PlatformError> {
    if let Some(value) = value.as_object_mut().ok_or_else(denied)?.get_mut(key) {
        let engine = value.as_str().ok_or_else(denied)?;
        if !engine.is_empty() {
            *value = frame(namespace, engine).into();
        }
    }
    Ok(())
}

fn frame_tree(value: &mut Value, namespace: &mut Namespace) -> Result<(), PlatformError> {
    let current = value.get_mut("frame").ok_or_else(denied)?;
    field(current, "id", namespace)?;
    field(current, "parentId", namespace)?;
    if let Some(children) = value.get_mut("childFrames") {
        for child in children.as_array_mut().ok_or_else(denied)? {
            frame_tree(child, namespace)?;
        }
    }
    Ok(())
}

fn node(value: &mut Value, namespace: &mut Namespace) -> Result<(), PlatformError> {
    field(value, "frameId", namespace)?;
    for key in ["children", "shadowRoots", "pseudoElements"] {
        if let Some(children) = value.get_mut(key) {
            for child in children.as_array_mut().ok_or_else(denied)? {
                node(child, namespace)?;
            }
        }
    }
    if let Some(document) = value.get_mut("contentDocument") {
        node(document, namespace)?;
    }
    Ok(())
}

pub(super) fn reply(
    method: &str,
    result: &mut Map<String, Value>,
    namespace: &mut Namespace,
) -> Result<(), PlatformError> {
    match method {
        "Page.getFrameTree" => {
            frame_tree(result.get_mut("frameTree").ok_or_else(denied)?, namespace)
        }
        "Page.navigate" => {
            if let Some(value) = result.get_mut("frameId") {
                *value = frame(namespace, value.as_str().ok_or_else(denied)?).into();
            }
            Ok(())
        }
        "DOM.getDocument" => node(result.get_mut("root").ok_or_else(denied)?, namespace),
        "DOM.describeNode" => node(result.get_mut("node").ok_or_else(denied)?, namespace),
        _ => Ok(()),
    }
}

pub(super) fn event(
    method: &str,
    params: &mut Value,
    namespace: &mut Namespace,
) -> Result<(), PlatformError> {
    if matches!(
        method.split_once('.').map(|(domain, _)| domain),
        Some("Page" | "Network" | "Fetch")
    ) {
        field(params, "frameId", namespace)?;
        field(params, "parentFrameId", namespace)?;
    }
    match method {
        "Page.frameNavigated" => {
            let current = params.get_mut("frame").ok_or_else(denied)?;
            field(current, "id", namespace)?;
            field(current, "parentId", namespace)
        }
        "Runtime.executionContextCreated" => {
            if let Some(data) = params.pointer_mut("/context/auxData") {
                field(data, "frameId", namespace)?;
            }
            Ok(())
        }
        "DOM.setChildNodes" => {
            for child in params
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .ok_or_else(denied)?
            {
                node(child, namespace)?;
            }
            Ok(())
        }
        "DOM.childNodeInserted" => node(params.get_mut("node").ok_or_else(denied)?, namespace),
        _ => Ok(()),
    }
}
