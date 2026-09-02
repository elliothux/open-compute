use crate::{MAX_METADATA_BYTES, MAX_METADATA_PREDICATES, SearchError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeSet;

const MAX_FILTER_LIST_VALUES: usize = 100;
const MAX_PROPERTY_PATH_BYTES: usize = 256;
const MAX_FILTER_JSON_BYTES: usize = 2_048;

/// One supported metadata scalar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataScalar {
    /// Null is accepted only in filter operands and denotes a missing property.
    Null,
    /// UTF-8 string.
    String(String),
    /// Finite JSON number represented as f64 for comparison.
    Number(f64),
    /// Boolean value.
    Boolean(bool),
}

/// One supported metadata field value.
#[derive(Clone, Debug, PartialEq)]
pub enum MetadataValue {
    /// Scalar value.
    Scalar(MetadataScalar),
    /// Non-empty list of strings.
    StringList(Vec<String>),
}

/// Canonical, validated vector metadata object.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedMetadata {
    canonical_json: Vec<u8>,
    value: Value,
}

impl IndexedMetadata {
    /// Canonical JSON bytes persisted as metadata authority.
    #[must_use]
    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical_json
    }

    /// Resolve a validated dot property path.
    #[must_use]
    pub fn get(&self, property_path: &str) -> Option<MetadataValue> {
        let mut value = &self.value;
        for component in property_path.split('.') {
            value = value.as_object()?.get(component)?;
        }
        parse_metadata_value(value).ok()
    }
}

/// Validate metadata shape, numeric finiteness, and the canonical 10 KiB limit.
pub fn validate_metadata(value: &Value) -> Result<IndexedMetadata, SearchError> {
    let object = value.as_object().ok_or(SearchError::InvalidMetadata)?;
    validate_object(object, 0)?;
    let canonical_value = canonicalize(value);
    let canonical_json =
        serde_json::to_vec(&canonical_value).map_err(|_| SearchError::InvalidMetadata)?;
    if canonical_json.len() > MAX_METADATA_BYTES {
        return Err(SearchError::InvalidMetadata);
    }
    Ok(IndexedMetadata {
        canonical_json,
        value: canonical_value,
    })
}

/// Supported metadata predicate operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterOperator {
    /// Equal or array contains.
    Equal,
    /// Missing field or a value that does not equal or contain the operand.
    NotEqual,
    /// Numeric or string less-than.
    LessThan,
    /// Numeric or string less-than-or-equal.
    LessThanOrEqual,
    /// Numeric or string greater-than.
    GreaterThan,
    /// Numeric or string greater-than-or-equal.
    GreaterThanOrEqual,
    /// Equal to any operand.
    In,
    /// Missing field or a value that equals none of the operands.
    NotIn,
}

/// One closed metadata field predicate.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterPredicate {
    property_path: String,
    operator: FilterOperator,
    operands: Vec<MetadataScalar>,
}

impl FilterPredicate {
    /// Indexed dot-property path evaluated by this predicate.
    #[must_use]
    pub fn property_path(&self) -> &str {
        &self.property_path
    }

    /// Closed comparison operator.
    #[must_use]
    pub const fn operator(&self) -> FilterOperator {
        self.operator
    }

    /// Validated scalar operands in request order.
    #[must_use]
    pub fn operands(&self) -> &[MetadataScalar] {
        &self.operands
    }
}

/// Validated implicit-AND metadata filter.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterExpr {
    predicates: Vec<FilterPredicate>,
}

impl FilterExpr {
    /// Evaluate against already validated metadata after index candidate selection.
    #[must_use]
    pub fn matches(&self, metadata: &IndexedMetadata) -> bool {
        self.predicates
            .iter()
            .all(|predicate| predicate.matches(metadata))
    }

    /// Number of implicit-AND field predicates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.predicates.len()
    }

    /// Whether the filter has no predicates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }

    /// Validated predicates for authority-owned index candidate pushdown.
    #[must_use]
    pub fn predicates(&self) -> &[FilterPredicate] {
        &self.predicates
    }
}

/// Compile a public filter object, requiring every property path to be indexed.
pub fn compile_filter(
    value: &Value,
    indexed_properties: &BTreeSet<String>,
) -> Result<FilterExpr, SearchError> {
    let object = value.as_object().ok_or(SearchError::InvalidFilter)?;
    let compact = serde_json::to_vec(value).map_err(|_| SearchError::InvalidFilter)?;
    if object.is_empty()
        || object.len() > MAX_METADATA_PREDICATES
        || compact.len() >= MAX_FILTER_JSON_BYTES
    {
        return Err(SearchError::InvalidFilter);
    }
    let mut predicates = Vec::with_capacity(object.len());
    for (path, expression) in object {
        validate_property_path(path)?;
        if !indexed_properties.contains(path) {
            return Err(SearchError::InvalidFilter);
        }
        predicates.extend(compile_predicates(path, expression)?);
        if predicates.len() > MAX_METADATA_PREDICATES {
            return Err(SearchError::InvalidFilter);
        }
    }
    Ok(FilterExpr { predicates })
}

impl FilterPredicate {
    fn matches(&self, metadata: &IndexedMetadata) -> bool {
        let value = metadata.get(&self.property_path);
        match self.operator {
            FilterOperator::Equal => equal(value.as_ref(), &self.operands[0]),
            FilterOperator::NotEqual => value
                .as_ref()
                .is_none_or(|value| !equal(Some(value), &self.operands[0])),
            FilterOperator::In => self
                .operands
                .iter()
                .any(|operand| equal(value.as_ref(), operand)),
            FilterOperator::NotIn => value.as_ref().is_none_or(|value| {
                self.operands
                    .iter()
                    .all(|operand| !equal(Some(value), operand))
            }),
            operator => ordered(value.as_ref(), &self.operands[0], operator),
        }
    }
}

fn compile_predicates(path: &str, value: &Value) -> Result<Vec<FilterPredicate>, SearchError> {
    if let Some(object) = value.as_object() {
        if object.is_empty() || object.len() > 2 || !valid_operator_set(object) {
            return Err(SearchError::InvalidFilter);
        }
        object
            .iter()
            .map(|(operator, operand)| {
                let operator = parse_operator(operator)?;
                let operands = if matches!(operator, FilterOperator::In | FilterOperator::NotIn) {
                    let values = operand.as_array().ok_or(SearchError::InvalidFilter)?;
                    if values.is_empty() || values.len() > MAX_FILTER_LIST_VALUES {
                        return Err(SearchError::InvalidFilter);
                    }
                    values
                        .iter()
                        .map(parse_filter_scalar)
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    vec![parse_filter_scalar(operand)?]
                };
                if matches!(
                    operator,
                    FilterOperator::LessThan
                        | FilterOperator::LessThanOrEqual
                        | FilterOperator::GreaterThan
                        | FilterOperator::GreaterThanOrEqual
                ) && matches!(
                    operands[0],
                    MetadataScalar::Null | MetadataScalar::Boolean(_)
                ) {
                    return Err(SearchError::InvalidFilter);
                }
                Ok(FilterPredicate {
                    property_path: path.to_string(),
                    operator,
                    operands,
                })
            })
            .collect()
    } else {
        Ok(vec![FilterPredicate {
            property_path: path.to_string(),
            operator: FilterOperator::Equal,
            operands: vec![parse_filter_scalar(value)?],
        }])
    }
}

fn valid_operator_set(object: &Map<String, Value>) -> bool {
    if object.len() == 1 {
        return true;
    }
    let lower = object
        .keys()
        .filter(|key| matches!(key.as_str(), "$gt" | "$gte"))
        .count();
    let upper = object
        .keys()
        .filter(|key| matches!(key.as_str(), "$lt" | "$lte"))
        .count();
    lower == 1 && upper == 1
}

fn parse_operator(value: &str) -> Result<FilterOperator, SearchError> {
    match value {
        "$eq" => Ok(FilterOperator::Equal),
        "$ne" => Ok(FilterOperator::NotEqual),
        "$lt" => Ok(FilterOperator::LessThan),
        "$lte" => Ok(FilterOperator::LessThanOrEqual),
        "$gt" => Ok(FilterOperator::GreaterThan),
        "$gte" => Ok(FilterOperator::GreaterThanOrEqual),
        "$in" => Ok(FilterOperator::In),
        "$nin" => Ok(FilterOperator::NotIn),
        _ => Err(SearchError::InvalidFilter),
    }
}

fn parse_filter_scalar(value: &Value) -> Result<MetadataScalar, SearchError> {
    match value {
        Value::Null => Ok(MetadataScalar::Null),
        Value::String(value) => Ok(MetadataScalar::String(value.clone())),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(MetadataScalar::Number)
            .ok_or(SearchError::InvalidFilter),
        Value::Bool(value) => Ok(MetadataScalar::Boolean(*value)),
        Value::Array(_) | Value::Object(_) => Err(SearchError::InvalidFilter),
    }
}

fn validate_object(object: &Map<String, Value>, depth: usize) -> Result<(), SearchError> {
    if depth > 1 {
        return Err(SearchError::InvalidMetadata);
    }
    for (key, value) in object {
        if key.is_empty() || key.len() > MAX_PROPERTY_PATH_BYTES || key.contains('.') {
            return Err(SearchError::InvalidMetadata);
        }
        if let Some(nested) = value.as_object() {
            validate_object(nested, depth + 1)?;
        } else {
            let _ = parse_metadata_value(value)?;
        }
    }
    Ok(())
}

fn parse_metadata_value(value: &Value) -> Result<MetadataValue, SearchError> {
    match value {
        Value::String(value) => Ok(MetadataValue::Scalar(MetadataScalar::String(value.clone()))),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| MetadataValue::Scalar(MetadataScalar::Number(value)))
            .ok_or(SearchError::InvalidMetadata),
        Value::Bool(value) => Ok(MetadataValue::Scalar(MetadataScalar::Boolean(*value))),
        Value::Array(values) if !values.is_empty() && values.iter().all(Value::is_string) => {
            Ok(MetadataValue::StringList(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            ))
        }
        Value::Null | Value::Array(_) | Value::Object(_) => Err(SearchError::InvalidMetadata),
    }
}

fn validate_property_path(path: &str) -> Result<(), SearchError> {
    if path.is_empty()
        || path.len() > MAX_PROPERTY_PATH_BYTES
        || path
            .split('.')
            .any(|component| component.is_empty() || component.starts_with('$'))
    {
        return Err(SearchError::InvalidFilter);
    }
    Ok(())
}

fn equal(value: Option<&MetadataValue>, operand: &MetadataScalar) -> bool {
    match (value, operand) {
        (None, MetadataScalar::Null) => true,
        (Some(MetadataValue::Scalar(value)), operand) => {
            scalar_cmp(value, operand) == Some(Ordering::Equal)
        }
        (Some(MetadataValue::StringList(values)), MetadataScalar::String(operand)) => {
            values.iter().any(|value| value == operand)
        }
        _ => false,
    }
}

fn ordered(
    value: Option<&MetadataValue>,
    operand: &MetadataScalar,
    operator: FilterOperator,
) -> bool {
    let Some(MetadataValue::Scalar(value)) = value else {
        return false;
    };
    let Some(ordering) = scalar_cmp(value, operand) else {
        return false;
    };
    match operator {
        FilterOperator::LessThan => ordering == Ordering::Less,
        FilterOperator::LessThanOrEqual => ordering != Ordering::Greater,
        FilterOperator::GreaterThan => ordering == Ordering::Greater,
        FilterOperator::GreaterThanOrEqual => ordering != Ordering::Less,
        FilterOperator::Equal
        | FilterOperator::NotEqual
        | FilterOperator::In
        | FilterOperator::NotIn => false,
    }
}

fn scalar_cmp(left: &MetadataScalar, right: &MetadataScalar) -> Option<Ordering> {
    match (left, right) {
        (MetadataScalar::Null, MetadataScalar::Null) => Some(Ordering::Equal),
        (MetadataScalar::String(left), MetadataScalar::String(right)) => Some(left.cmp(right)),
        (MetadataScalar::Number(left), MetadataScalar::Number(right)) => left.partial_cmp(right),
        (MetadataScalar::Boolean(left), MetadataScalar::Boolean(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key.clone(), canonicalize(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        value => value.clone(),
    }
}
