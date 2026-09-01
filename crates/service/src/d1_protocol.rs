//! Authenticated binary D1 transport framing with native BLOB fields.

use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::{
    D1_MAX_BATCH_STATEMENTS, D1_MAX_BOUND_PARAMS, D1_MAX_SQL_BYTES, D1Statement, D1StatementResult,
    D1Value,
};

pub(crate) const D1_FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.d1.v1+frame";
pub(crate) const D1_JSON_CONTENT_TYPE: &str = "application/vnd.open-compute.d1.v1+json";
pub(crate) const D1_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const D1_MAX_BOOKMARK_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum D1QueryMode {
    All,
    Run,
    Raw,
    Batch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum D1SessionConstraint {
    AlwaysPrimary,
    FirstUnconstrained,
    FirstPrimary,
    Bookmark(String),
}

#[derive(Debug)]
pub(crate) struct D1QueryRequest {
    pub(crate) mode: D1QueryMode,
    pub(crate) statements: Vec<D1Statement>,
    pub(crate) session: D1SessionConstraint,
}

pub(crate) fn decode_query(bytes: &[u8]) -> Result<D1QueryRequest, PlatformError> {
    if bytes.len() > D1_MAX_FRAME_BYTES {
        return Err(limit_error());
    }
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != b"D1Q1" {
        return Err(protocol_error());
    }
    let mode = match reader.u8()? {
        1 => D1QueryMode::All,
        2 => D1QueryMode::Run,
        3 => D1QueryMode::Raw,
        4 => D1QueryMode::Batch,
        _ => return Err(protocol_error()),
    };
    let count = usize::from(reader.u16()?);
    if count == 0 || count > D1_MAX_BATCH_STATEMENTS || (mode != D1QueryMode::Batch && count != 1) {
        return Err(protocol_error());
    }
    let mut statements = Vec::with_capacity(count);
    for _ in 0..count {
        let sql = reader.text(D1_MAX_SQL_BYTES)?;
        let parameter_count = usize::from(reader.u16()?);
        if parameter_count > D1_MAX_BOUND_PARAMS {
            return Err(limit_error());
        }
        let mut params = Vec::with_capacity(parameter_count);
        for _ in 0..parameter_count {
            params.push(reader.value()?);
        }
        statements.push(D1Statement { sql, params });
    }
    let session = match reader.u8()? {
        0 => D1SessionConstraint::AlwaysPrimary,
        1 => D1SessionConstraint::FirstUnconstrained,
        2 => D1SessionConstraint::FirstPrimary,
        3 => {
            let token = reader.text(D1_MAX_BOOKMARK_CHARS)?;
            if token.is_empty() {
                return Err(protocol_error());
            }
            D1SessionConstraint::Bookmark(token)
        }
        _ => return Err(protocol_error()),
    };
    reader.done()?;
    Ok(D1QueryRequest {
        mode,
        statements,
        session,
    })
}

pub(crate) fn decode_exec(bytes: &[u8]) -> Result<String, PlatformError> {
    if bytes.len() > D1_MAX_FRAME_BYTES {
        return Err(limit_error());
    }
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != b"D1E1" {
        return Err(protocol_error());
    }
    let sql = reader.text(D1_MAX_SQL_BYTES)?;
    reader.done()?;
    Ok(sql)
}

pub(crate) fn encode_results(
    results: &[D1StatementResult],
    bookmark: Option<&str>,
    session_version: u64,
) -> Result<Vec<u8>, PlatformError> {
    let mut writer = Writer::new();
    writer.bytes(b"D1R1")?;
    writer.u16(u16::try_from(results.len()).map_err(|_| protocol_error())?)?;
    for result in results {
        writer.u16(u16::try_from(result.columns.len()).map_err(|_| protocol_error())?)?;
        for column in &result.columns {
            writer.text(column)?;
        }
        writer.u32(u32::try_from(result.rows.len()).map_err(|_| limit_error())?)?;
        for row in &result.rows {
            if row.len() != result.columns.len() {
                return Err(protocol_error());
            }
            for value in row {
                writer.value(value)?;
            }
        }
        let meta = serde_json::to_vec(&result.meta).map_err(|_| protocol_error())?;
        writer.length_bytes(&meta)?;
    }
    writer.text(bookmark.unwrap_or(""))?;
    writer.u64(session_version)?;
    Ok(writer.finish())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], PlatformError> {
        let end = self.offset.checked_add(length).ok_or_else(protocol_error)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(protocol_error)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, PlatformError> {
        self.take(1)?.first().copied().ok_or_else(protocol_error)
    }

    fn u16(&mut self) -> Result<u16, PlatformError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().map_err(|_| protocol_error())?,
        ))
    }

    fn u32(&mut self) -> Result<u32, PlatformError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| protocol_error())?,
        ))
    }

    fn i64(&mut self) -> Result<i64, PlatformError> {
        Ok(i64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| protocol_error())?,
        ))
    }

    fn f64(&mut self) -> Result<f64, PlatformError> {
        Ok(f64::from_bits(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| protocol_error())?,
        )))
    }

    fn length_bytes(&mut self, maximum: usize) -> Result<&'a [u8], PlatformError> {
        let length = usize::try_from(self.u32()?).map_err(|_| protocol_error())?;
        if length > maximum {
            return Err(limit_error());
        }
        self.take(length)
    }

    fn text(&mut self, maximum: usize) -> Result<String, PlatformError> {
        std::str::from_utf8(self.length_bytes(maximum)?)
            .map(str::to_owned)
            .map_err(|_| protocol_error())
    }

    fn value(&mut self) -> Result<D1Value, PlatformError> {
        match self.u8()? {
            0 => Ok(D1Value::Null),
            1 => Ok(D1Value::Integer(self.i64()?)),
            2 => {
                let value = self.f64()?;
                if !value.is_finite() {
                    return Err(PlatformError::new(
                        ErrorCode::D1TypeError,
                        "D1 numbers must be finite",
                    ));
                }
                Ok(D1Value::Real(value))
            }
            3 => self
                .text(open_compute_storage::D1_MAX_VALUE_OR_ROW_BYTES)
                .map(D1Value::Text),
            4 => self
                .length_bytes(open_compute_storage::D1_MAX_VALUE_OR_ROW_BYTES)
                .map(|value| D1Value::Blob(value.to_vec())),
            _ => Err(protocol_error()),
        }
    }

    fn done(self) -> Result<(), PlatformError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(protocol_error())
        }
    }
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn reserve(&self, added: usize) -> Result<(), PlatformError> {
        if self
            .bytes
            .len()
            .checked_add(added)
            .is_none_or(|size| size > D1_MAX_FRAME_BYTES)
        {
            return Err(limit_error());
        }
        Ok(())
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), PlatformError> {
        self.reserve(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<(), PlatformError> {
        self.bytes(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<(), PlatformError> {
        self.bytes(&value.to_be_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), PlatformError> {
        self.bytes(&value.to_be_bytes())
    }
    fn i64(&mut self, value: i64) -> Result<(), PlatformError> {
        self.bytes(&value.to_be_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), PlatformError> {
        self.bytes(&value.to_be_bytes())
    }
    fn f64(&mut self, value: f64) -> Result<(), PlatformError> {
        self.bytes(&value.to_bits().to_be_bytes())
    }
    fn length_bytes(&mut self, value: &[u8]) -> Result<(), PlatformError> {
        self.u32(u32::try_from(value.len()).map_err(|_| limit_error())?)?;
        self.bytes(value)
    }
    fn text(&mut self, value: &str) -> Result<(), PlatformError> {
        self.length_bytes(value.as_bytes())
    }
    fn value(&mut self, value: &D1Value) -> Result<(), PlatformError> {
        match value {
            D1Value::Null => self.u8(0),
            D1Value::Integer(value) => {
                self.u8(1)?;
                self.i64(*value)
            }
            D1Value::Real(value) if value.is_finite() => {
                self.u8(2)?;
                self.f64(*value)
            }
            D1Value::Real(_) => Err(protocol_error()),
            D1Value::Text(value) => {
                self.u8(3)?;
                self.text(value)
            }
            D1Value::Blob(value) => {
                self.u8(4)?;
                self.length_bytes(value)
            }
        }
    }
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1InternalProtocolError,
        "D1 private protocol frame is invalid",
    )
}

fn limit_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1LimitError,
        "D1 private frame exceeded its fixed limit",
    )
}

#[cfg(test)]
#[path = "d1_protocol_tests.rs"]
mod tests;
