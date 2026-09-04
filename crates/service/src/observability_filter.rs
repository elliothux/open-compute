//! Shared bounded Workers Observability filter AST and evaluator.

use open_compute_core::{ErrorCode, PlatformError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_FILTER_DEPTH: u8 = 4;
const MAX_FILTERS: usize = 32;
const MAX_REGEX_BYTES: usize = 512;

/// Logical combination for one filter list.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Combination {
    /// Every child must match.
    #[default]
    And,
    /// At least one child must match.
    Or,
    /// Cloudflare's accepted uppercase `AND` alias.
    #[serde(rename = "AND")]
    UpperAnd,
    /// Cloudflare's accepted uppercase `OR` alias.
    #[serde(rename = "OR")]
    UpperOr,
}

impl Combination {
    fn any(self) -> bool {
        matches!(self, Self::Or | Self::UpperOr)
    }
}

/// Recursive filter node accepted by query and Live Tail.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum FilterNode {
    /// Nested group.
    Group(FilterGroup),
    /// Scalar leaf.
    Leaf(FilterLeaf),
}

/// Nested filter group.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FilterGroup {
    kind: GroupKind,
    pub(crate) filter_combination: Combination,
    pub(crate) filters: Vec<FilterNode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum GroupKind {
    Group,
}

/// Scalar filter leaf.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FilterLeaf {
    pub(crate) key: String,
    operation: String,
    #[serde(rename = "type")]
    value_type: ScalarType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<LeafKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum LeafKind {
    Filter,
}

/// Supported scalar field kinds.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ScalarType {
    /// JSON string.
    String,
    /// JSON number.
    Number,
    /// JSON boolean.
    Boolean,
}

impl ScalarType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }
}

pub(crate) fn validate(filters: &[FilterNode]) -> Result<(), PlatformError> {
    let mut count = 0;
    for node in filters {
        validate_node(node, 1, &mut count)?;
    }
    if count > MAX_FILTERS {
        return Err(invalid());
    }
    Ok(())
}

fn validate_node(node: &FilterNode, depth: u8, count: &mut usize) -> Result<(), PlatformError> {
    if depth > MAX_FILTER_DEPTH {
        return Err(invalid());
    }
    *count += 1;
    match node {
        FilterNode::Group(group) => {
            if group.filters.is_empty() {
                return Err(invalid());
            }
            for child in &group.filters {
                validate_node(child, depth + 1, count)?;
            }
        }
        FilterNode::Leaf(leaf) => {
            if leaf.key.is_empty() || leaf.key.len() > 512 {
                return Err(invalid());
            }
            let operation = normalized_operation(&leaf.operation).ok_or_else(invalid)?;
            match operation {
                "exists" | "is_null" => {
                    if leaf.value.is_some() {
                        return Err(invalid());
                    }
                }
                _ => {
                    let value = leaf.value.as_ref().ok_or_else(invalid)?;
                    if scalar_type(value) != Some(leaf.value_type) {
                        return Err(invalid());
                    }
                    if matches!(
                        operation,
                        "includes"
                            | "not_includes"
                            | "starts_with"
                            | "ends_with"
                            | "regex"
                            | "in"
                            | "not_in"
                    ) && leaf.value_type != ScalarType::String
                    {
                        return Err(invalid());
                    }
                    if matches!(operation, "gt" | "gte" | "lt" | "lte")
                        && leaf.value_type != ScalarType::Number
                    {
                        return Err(invalid());
                    }
                    if operation == "regex" {
                        let pattern = value.as_str().ok_or_else(invalid)?;
                        if pattern.len() > MAX_REGEX_BYTES || Regex::new(pattern).is_err() {
                            return Err(invalid());
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn matches(
    filters: &[FilterNode],
    combination: Combination,
    event: &Value,
) -> Result<bool, PlatformError> {
    if filters.is_empty() {
        return Ok(true);
    }
    if combination.any() {
        for filter in filters {
            if matches_node(filter, event)? {
                return Ok(true);
            }
        }
        Ok(false)
    } else {
        for filter in filters {
            if !matches_node(filter, event)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn matches_node(node: &FilterNode, event: &Value) -> Result<bool, PlatformError> {
    match node {
        FilterNode::Group(group) => matches(&group.filters, group.filter_combination, event),
        FilterNode::Leaf(leaf) => matches_leaf(leaf, event),
    }
}

fn matches_leaf(leaf: &FilterLeaf, event: &Value) -> Result<bool, PlatformError> {
    let actual = field_value(event, &leaf.key);
    let operation = normalized_operation(&leaf.operation).ok_or_else(invalid)?;
    if operation == "exists" {
        return Ok(actual.is_some_and(|value| !value.is_null()));
    }
    if operation == "is_null" {
        return Ok(actual.is_none_or(Value::is_null));
    }
    let Some(actual) = actual else {
        return Ok(false);
    };
    if scalar_type(actual) != Some(leaf.value_type) {
        return Ok(false);
    }
    let expected = leaf.value.as_ref().ok_or_else(invalid)?;
    let equal = actual == expected;
    Ok(match operation {
        "eq" => equal,
        "neq" => !equal,
        "includes" => strings(actual, expected, |left, right| left.contains(right)),
        "not_includes" => !strings(actual, expected, |left, right| left.contains(right)),
        "starts_with" => strings(actual, expected, |left, right| left.starts_with(right)),
        "ends_with" => strings(actual, expected, |left, right| left.ends_with(right)),
        "regex" => expected
            .as_str()
            .and_then(|pattern| Regex::new(pattern).ok())
            .zip(actual.as_str())
            .is_some_and(|(regex, value)| regex.is_match(value)),
        "in" | "not_in" => {
            let member = expected.as_str().is_some_and(|values| {
                values
                    .split(',')
                    .any(|value| actual.as_str().is_some_and(|actual| actual == value.trim()))
            });
            if operation == "in" { member } else { !member }
        }
        "gt" | "gte" | "lt" | "lte" => {
            let Some((left, right)) = actual.as_f64().zip(expected.as_f64()) else {
                return Ok(false);
            };
            match operation {
                "gt" => left > right,
                "gte" => left >= right,
                "lt" => left < right,
                "lte" => left <= right,
                _ => false,
            }
        }
        _ => false,
    })
}

fn strings(actual: &Value, expected: &Value, compare: impl Fn(&str, &str) -> bool) -> bool {
    actual
        .as_str()
        .zip(expected.as_str())
        .is_some_and(|(left, right)| compare(left, right))
}

fn normalized_operation(value: &str) -> Option<&'static str> {
    match value {
        "includes" | "INCLUDES" => Some("includes"),
        "not_includes" | "DOES_NOT_INCLUDE" => Some("not_includes"),
        "starts_with" | "STARTS_WITH" => Some("starts_with"),
        "ends_with" | "ENDS_WITH" => Some("ends_with"),
        "regex" | "MATCH_REGEX" => Some("regex"),
        "exists" | "EXISTS" => Some("exists"),
        "is_null" | "DOES_NOT_EXIST" => Some("is_null"),
        "in" | "IN" => Some("in"),
        "not_in" | "NOT_IN" => Some("not_in"),
        "eq" | "=" => Some("eq"),
        "neq" | "!=" => Some("neq"),
        "gt" | ">" => Some("gt"),
        "gte" | ">=" => Some("gte"),
        "lt" | "<" => Some("lt"),
        "lte" | "<=" => Some("lte"),
        _ => None,
    }
}

pub(crate) fn collect_keys(filters: &[FilterNode], keys: &mut BTreeSet<String>) {
    for filter in filters {
        match filter {
            FilterNode::Leaf(leaf) => {
                keys.insert(leaf.key.clone());
            }
            FilterNode::Group(group) => collect_keys(&group.filters, keys),
        }
    }
}

pub(crate) fn field_value<'a>(event: &'a Value, key: &str) -> Option<&'a Value> {
    let mut value = event;
    for part in key.split('.') {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

pub(crate) fn flatten_public(
    prefix: &str,
    value: &Value,
    output: &mut BTreeMap<String, Value>,
    depth: u8,
) {
    if output.len() >= 256 || depth >= 32 {
        return;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            if !prefix.is_empty() {
                output
                    .entry(prefix.to_owned())
                    .or_insert_with(|| value.clone());
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_public(&next, value, output, depth + 1);
            }
        }
        Value::Array(_) => {}
    }
}

pub(crate) fn scalar_type(value: &Value) -> Option<ScalarType> {
    match value {
        Value::String(_) => Some(ScalarType::String),
        Value::Number(_) => Some(ScalarType::Number),
        Value::Bool(_) => Some(ScalarType::Boolean),
        _ => None,
    }
}

pub(crate) fn scalar_kind(value: &Value) -> Option<&'static str> {
    scalar_type(value).map(ScalarType::as_str)
}

fn invalid() -> PlatformError {
    PlatformError::new(ErrorCode::LimitInvalid, "observability filter is invalid")
}

#[cfg(test)]
#[path = "observability_filter_tests.rs"]
mod tests;
