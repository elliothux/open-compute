//! Bounded canonical JSON shared by Workflow payload and result authority.

use super::{WORKFLOW_JSON_MAX_BYTES, WORKFLOW_JSON_MAX_DEPTH, error};
use crate::{ErrorCode, PlatformError};
use serde_json::value::RawValue;

/// Parse and canonicalize a capability V1 JSON string at the authority boundary.
/// JSON numbers have JavaScript Number semantics, not arbitrary-precision integers.
pub fn canonical_json(input: &str, size_error: ErrorCode) -> Result<String, PlatformError> {
    let value = decode_json(input, size_error)?;
    let mut output = String::new();
    write_value(&value, &mut output)?;
    if output.len() > WORKFLOW_JSON_MAX_BYTES {
        return Err(error(size_error));
    }
    Ok(output)
}

/// Decode bounded Workflow JSON with correctly rounded JavaScript Number semantics.
/// Use this when embedding durable results in another JSON response; the default
/// `serde_json` number parser does not guarantee correctly rounded binary64 values.
pub fn decode_json(input: &str, size_error: ErrorCode) -> Result<serde_json::Value, PlatformError> {
    if input.len() > WORKFLOW_JSON_MAX_BYTES {
        return Err(error(size_error));
    }
    let value: &RawValue = serde_json::from_str(input)
        .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?;
    read_value(value, 0)
}

fn read_value(value: &RawValue, depth: usize) -> Result<serde_json::Value, PlatformError> {
    use serde_json::Value;
    let raw = value.get();
    let invalid = || error(ErrorCode::WorkflowSerializationUnsupported);
    match raw.as_bytes().first() {
        Some(b'n') => Ok(Value::Null),
        Some(b't') => Ok(Value::Bool(true)),
        Some(b'f') => Ok(Value::Bool(false)),
        Some(b'"') => serde_json::from_str(raw)
            .map(Value::String)
            .map_err(|_| invalid()),
        Some(b'[') => {
            if depth >= WORKFLOW_JSON_MAX_DEPTH {
                return Err(invalid());
            }
            let values: Vec<&RawValue> = serde_json::from_str(raw).map_err(|_| invalid())?;
            values
                .into_iter()
                .map(|value| read_value(value, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some(b'{') => {
            if depth >= WORKFLOW_JSON_MAX_DEPTH {
                return Err(invalid());
            }
            let values: std::collections::BTreeMap<String, &RawValue> =
                serde_json::from_str(raw).map_err(|_| invalid())?;
            values
                .into_iter()
                .map(|(key, value)| Ok((key, read_value(value, depth + 1)?)))
                .collect::<Result<serde_json::Map<_, _>, _>>()
                .map(Value::Object)
        }
        _ => {
            let value = raw.parse::<f64>().map_err(|_| invalid())?;
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(invalid)
        }
    }
}

fn write_value(value: &serde_json::Value, output: &mut String) -> Result<(), PlatformError> {
    use serde_json::Value;
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?,
        ),
        Value::Number(value) => output.push_str(&number(
            value
                .as_f64()
                .ok_or_else(|| error(ErrorCode::WorkflowSerializationUnsupported))?,
        )?),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_value(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?,
                );
                output.push(':');
                write_value(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}
fn number(value: f64) -> Result<String, PlatformError> {
    if value == 0.0 {
        return Ok("0".into());
    }
    if !value.is_finite() {
        return Err(error(ErrorCode::WorkflowSerializationUnsupported));
    }
    // Reuse serde_json's shortest-roundtrip digits, changing only the ECMAScript
    // decimal/exponent presentation thresholds. No independent float formatter.
    let raw = serde_json::to_string(&value.abs())
        .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?;
    let (mantissa, exponent) =
        raw.split_once('e')
            .map_or(Ok((raw.as_str(), 0_i32)), |(mantissa, exp)| {
                exp.parse::<i32>()
                    .map(|exp| (mantissa, exp))
                    .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))
            })?;
    let decimal = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digits = mantissa.replace('.', "");
    let leading = digits.bytes().take_while(|byte| *byte == b'0').count();
    let position = i32::try_from(decimal)
        .map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?
        + exponent
        - i32::try_from(leading).map_err(|_| error(ErrorCode::WorkflowSerializationUnsupported))?;
    digits.drain(..leading);
    while digits.ends_with('0') {
        digits.pop();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if (1..=21).contains(&position) {
        let position = position as usize;
        if position >= digits.len() {
            return Ok(format!(
                "{sign}{digits}{}",
                "0".repeat(position - digits.len())
            ));
        }
        return Ok(format!(
            "{sign}{}.{}",
            &digits[..position],
            &digits[position..]
        ));
    }
    if (-5..=0).contains(&position) {
        return Ok(format!(
            "{sign}0.{}{digits}",
            "0".repeat((-position) as usize)
        ));
    }
    let tail = if digits.len() == 1 {
        String::new()
    } else {
        format!(".{}", &digits[1..])
    };
    let exponent = position - 1;
    Ok(format!(
        "{sign}{}{tail}e{}{exponent}",
        &digits[..1],
        if exponent >= 0 { "+" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_subset_matches_javascript_fixtures() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            input: String,
            expected: String,
        }
        let fixtures: Vec<Fixture> = serde_json::from_str(include_str!(
            "../../../../runtime/tests/fixtures/workflow-json.json"
        ))
        .unwrap();
        for fixture in fixtures {
            assert_eq!(
                canonical_json(&fixture.input, ErrorCode::WorkflowPayloadTooLarge).unwrap(),
                fixture.expected,
                "{}",
                fixture.input
            );
            let decoded =
                decode_json(&fixture.expected, ErrorCode::WorkflowPayloadTooLarge).unwrap();
            let response = serde_json::to_string(&decoded).unwrap();
            assert_eq!(
                canonical_json(&response, ErrorCode::WorkflowPayloadTooLarge).unwrap(),
                fixture.expected
            );
        }
    }

    #[test]
    fn canonical_json_enforces_input_and_container_bounds() {
        let limit = format!("\"{}\"", "a".repeat(WORKFLOW_JSON_MAX_BYTES - 2));
        assert_eq!(
            canonical_json(&limit, ErrorCode::WorkflowPayloadTooLarge)
                .unwrap()
                .len(),
            WORKFLOW_JSON_MAX_BYTES
        );
        assert_eq!(
            canonical_json(&(limit + " "), ErrorCode::WorkflowPayloadTooLarge)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowPayloadTooLarge
        );
        for invalid in ["undefined", "1e999", r#""\ud800""#, "{", "[NaN]"] {
            assert!(canonical_json(invalid, ErrorCode::WorkflowPayloadTooLarge).is_err());
        }
        let deep = format!("{}null{}", "[".repeat(127), "]".repeat(127));
        canonical_json(&deep, ErrorCode::WorkflowPayloadTooLarge).unwrap();
        assert!(canonical_json(&format!("[{deep}]"), ErrorCode::WorkflowPayloadTooLarge).is_err());
    }
}
