//! Exact, bounded duration parsing shared with the current facade.

use super::error;
use crate::{ErrorCode, PlatformError};
use serde_json::Value;

/// Maximum integer that can cross the JavaScript/SQLite protocol without loss.
pub const WORKFLOW_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
/// Maximum one-shot durable sleep or event timeout: 365 days.
pub const WORKFLOW_MAX_DURATION_MS: u64 = 365 * 24 * 60 * 60 * 1000;

fn invalid() -> PlatformError {
    error(ErrorCode::WorkflowDurationInvalid)
}

// ECMAScript WhiteSpace + LineTerminator, kept identical to JS trim()/\s.
fn whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// Parse finite numeric milliseconds or a decimal followed by one supported unit.
/// Fractions round up; string arithmetic never passes through a floating-point parse.
/// The caller supplies its semantic cap (sleep, retry delay, retention, or timeout).
pub fn duration_ms(value: &Value, maximum: u64) -> Result<u64, PlatformError> {
    let parsed = match value {
        Value::Number(number) => {
            let number = number.as_f64().ok_or_else(invalid)?;
            if !number.is_finite()
                || number < 0.0
                || number.ceil() > WORKFLOW_MAX_SAFE_INTEGER as f64
            {
                return Err(invalid());
            }
            number.ceil() as u64
        }
        Value::String(value) => string_duration(value)?,
        _ => return Err(invalid()),
    };
    if parsed > maximum.min(WORKFLOW_MAX_SAFE_INTEGER) {
        return Err(invalid());
    }
    Ok(parsed)
}

fn string_duration(value: &str) -> Result<u64, PlatformError> {
    if value.len() > 4096 {
        return Err(invalid());
    }
    let value = value.trim_matches(whitespace);
    let separator = value.find(whitespace).ok_or_else(invalid)?;
    let decimal = &value[..separator];
    let unit = value[separator..]
        .trim_start_matches(whitespace)
        .to_ascii_lowercase();
    let multiplier: u64 = match unit.as_str() {
        "ms" | "millisecond" | "milliseconds" => 1,
        "s" | "second" | "seconds" => 1000,
        "m" | "minute" | "minutes" => 60_000,
        "h" | "hour" | "hours" => 3_600_000,
        "d" | "day" | "days" => 86_400_000,
        "w" | "week" | "weeks" => 604_800_000,
        _ => return Err(invalid()),
    };
    let (whole, fraction) = decimal.split_once('.').unwrap_or((decimal, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let mut integer = 0_u64;
    for digit in whole.bytes() {
        integer = integer
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(digit - b'0')))
            .filter(|value| *value <= WORKFLOW_MAX_SAFE_INTEGER)
            .ok_or_else(invalid)?;
    }
    // Decimal long multiplication from the least significant digit gives the
    // exact integer part and whether any discarded fractional digit was nonzero.
    let mut carry = 0_u64;
    let mut remainder = false;
    for digit in fraction.bytes().rev() {
        let product = u64::from(digit - b'0') * multiplier + carry;
        remainder |= !product.is_multiple_of(10);
        carry = product / 10;
    }
    integer
        .checked_mul(multiplier)
        .and_then(|value| value.checked_add(carry))
        .and_then(|value| value.checked_add(u64::from(remainder)))
        .filter(|value| *value <= WORKFLOW_MAX_SAFE_INTEGER)
        .ok_or_else(invalid)
}

/// Validate an absolute, integral Unix millisecond timestamp, including past dates.
/// Relative duration rounding must never be applied to a persisted absolute deadline.
pub fn timestamp_ms(value: &Value) -> Result<i64, PlatformError> {
    let number = value.as_f64().ok_or_else(invalid)?;
    if !number.is_finite()
        || number.fract() != 0.0
        || number.abs() > WORKFLOW_MAX_SAFE_INTEGER as f64
    {
        return Err(invalid());
    }
    Ok(number as i64)
}
