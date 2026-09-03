//! JSON subset conversion for the existing durable Workflow value wire.

use crate::cloudflare_v4::V4Error;
use base64::Engine as _;
use serde_json::{Map, Number, Value};

const HEADER: &[u8] = b"OCDV\x01\x02";
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 100_000;

pub(super) fn parameter(value: Value) -> Result<String, V4Error> {
    let value = match value {
        Value::String(encoded) => {
            serde_json::from_str(&encoded).map_err(|_| V4Error::InvalidRequest)?
        }
        value => value,
    };
    encode(&value)
}

pub(super) fn encode(value: &Value) -> Result<String, V4Error> {
    let mut bytes = HEADER.to_vec();
    encode_value(&mut bytes, value, 0)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub(super) fn decode(encoded: &str) -> Result<Value, V4Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| V4Error::Internal)?;
    if !bytes.starts_with(HEADER) {
        return Err(V4Error::Internal);
    }
    let mut reader = Reader {
        bytes: &bytes,
        offset: HEADER.len(),
        entries: 0,
    };
    let value = reader.value(0)?;
    if reader.offset != reader.bytes.len() {
        return Err(V4Error::Internal);
    }
    Ok(value)
}

fn encode_value(output: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), V4Error> {
    if depth > MAX_DEPTH {
        return Err(V4Error::InvalidRequest);
    }
    match value {
        Value::Null => output.push(0x00),
        Value::Bool(false) => output.push(0x02),
        Value::Bool(true) => output.push(0x03),
        Value::Number(number) => {
            let value = number.as_f64().ok_or(V4Error::InvalidRequest)?;
            if !value.is_finite() {
                return Err(V4Error::InvalidRequest);
            }
            output.push(0x04);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Value::String(text) => {
            output.push(0x06);
            write_string(output, text)?;
        }
        Value::Array(items) => {
            output.push(0x10);
            write_count(output, items.len())?;
            output.extend_from_slice(&0_u32.to_be_bytes());
            for item in items {
                encode_value(output, item, depth + 1)?;
            }
        }
        Value::Object(fields) => {
            output.push(0x11);
            write_count(output, fields.len())?;
            for (key, value) in fields {
                write_string(output, key)?;
                encode_value(output, value, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn write_count(output: &mut Vec<u8>, count: usize) -> Result<(), V4Error> {
    if count > MAX_ENTRIES {
        return Err(V4Error::InvalidRequest);
    }
    output.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| V4Error::InvalidRequest)?
            .to_be_bytes(),
    );
    Ok(())
}

fn write_string(output: &mut Vec<u8>, value: &str) -> Result<(), V4Error> {
    write_count(output, value.len())?;
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    entries: usize,
}

impl Reader<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, V4Error> {
        if depth > MAX_DEPTH {
            return Err(V4Error::Internal);
        }
        match self.byte()? {
            0x00 | 0x01 => Ok(Value::Null),
            0x02 => Ok(Value::Bool(false)),
            0x03 => Ok(Value::Bool(true)),
            0x04 => {
                let number =
                    f64::from_be_bytes(self.take(8)?.try_into().map_err(|_| V4Error::Internal)?);
                if number.fract() == 0.0 && number >= i64::MIN as f64 && number <= i64::MAX as f64 {
                    Ok(Value::Number(Number::from(number as i64)))
                } else {
                    Number::from_f64(number)
                        .map(Value::Number)
                        .ok_or(V4Error::Internal)
                }
            }
            0x06 => self.string().map(Value::String),
            0x10 => {
                let count = self.count()?;
                if self.count()? != 0 {
                    return Err(V4Error::Internal);
                }
                self.charge(count)?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            0x11 => {
                let count = self.count()?;
                self.charge(count)?;
                let mut fields = Map::new();
                for _ in 0..count {
                    let key = self.string()?;
                    if fields.contains_key(&key) {
                        return Err(V4Error::Internal);
                    }
                    fields.insert(key, self.value(depth + 1)?);
                }
                Ok(Value::Object(fields))
            }
            _ => Err(V4Error::Internal),
        }
    }

    fn byte(&mut self) -> Result<u8, V4Error> {
        Ok(self.take(1)?[0])
    }

    fn count(&mut self) -> Result<usize, V4Error> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| V4Error::Internal)?;
        usize::try_from(u32::from_be_bytes(bytes)).map_err(|_| V4Error::Internal)
    }

    fn string(&mut self) -> Result<String, V4Error> {
        let count = self.count()?;
        String::from_utf8(self.take(count)?.to_vec()).map_err(|_| V4Error::Internal)
    }

    fn take(&mut self, count: usize) -> Result<&[u8], V4Error> {
        let end = self.offset.checked_add(count).ok_or(V4Error::Internal)?;
        let bytes = self.bytes.get(self.offset..end).ok_or(V4Error::Internal)?;
        self.offset = end;
        Ok(bytes)
    }

    fn charge(&mut self, count: usize) -> Result<(), V4Error> {
        self.entries = self.entries.checked_add(count).ok_or(V4Error::Internal)?;
        if self.entries > MAX_ENTRIES {
            return Err(V4Error::Internal);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_subset_round_trips_the_runtime_wire() {
        let value = serde_json::json!({"ok":true,"items":[null,7,"x"]});
        assert_eq!(decode(&encode(&value).unwrap()).unwrap(), value);
        assert_eq!(encode(&Value::Null).unwrap(), "T0NEVgECAA==");
    }
}
