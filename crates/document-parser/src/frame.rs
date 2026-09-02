use crate::{
    DocumentErrorCode, DocumentParserError, InputHeader, MAX_DOCUMENT_BYTES, MAX_HEADER_BYTES,
    MAX_MARKDOWN_BYTES, MAX_OUTPUT_FRAME_BYTES, PARSER_CONTRACT_SHA256, PROTOCOL_VERSION,
    ParseOutput, ParseRequest, error, has_disallowed_control, sha256_hex, validate_metadata,
};

const MAGIC: &[u8; 4] = b"OCDP";
const PRELUDE_BYTES: usize = 14;

/// Encode one canonical OCDP v1 input frame.
pub fn encode_input_frame(request: &ParseRequest) -> Result<Vec<u8>, DocumentParserError> {
    validate_input(&request.header, &request.body)?;
    let header = serde_json::to_vec(&request.header)
        .map_err(|_| error(DocumentErrorCode::InvalidRequest))?;
    if header.len() > MAX_HEADER_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let mut output = Vec::with_capacity(PRELUDE_BYTES + header.len() + request.body.len());
    encode_prelude(&mut output, header.len(), request.body.len())?;
    output.extend_from_slice(&header);
    output.extend_from_slice(&request.body);
    Ok(output)
}

/// Decode and fully validate one canonical OCDP v1 input frame.
pub fn decode_input_frame(frame: &[u8]) -> Result<ParseRequest, DocumentParserError> {
    let (header_length, body_length) = decode_prelude(frame)?;
    if header_length > MAX_HEADER_BYTES || body_length > MAX_DOCUMENT_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let expected = PRELUDE_BYTES
        .checked_add(header_length)
        .and_then(|length| length.checked_add(body_length))
        .ok_or_else(|| error(DocumentErrorCode::InvalidFrame))?;
    if frame.len() != expected {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    let header_bytes = &frame[PRELUDE_BYTES..PRELUDE_BYTES + header_length];
    let header: InputHeader =
        serde_json::from_slice(header_bytes).map_err(|_| error(DocumentErrorCode::InvalidFrame))?;
    let canonical =
        serde_json::to_vec(&header).map_err(|_| error(DocumentErrorCode::InvalidFrame))?;
    if canonical != header_bytes {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    let body = frame[PRELUDE_BYTES + header_length..].to_vec();
    validate_input(&header, &body)?;
    Ok(ParseRequest { header, body })
}

/// Encode one canonical OCDP v1 output frame with an empty binary body.
pub fn encode_output_frame(output: &ParseOutput) -> Result<Vec<u8>, DocumentParserError> {
    validate_output(output)?;
    let json = serde_json::to_vec(output).map_err(|_| error(DocumentErrorCode::InvalidFrame))?;
    if json.len() > MAX_OUTPUT_FRAME_BYTES - PRELUDE_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let mut frame = Vec::with_capacity(PRELUDE_BYTES + json.len());
    encode_prelude(&mut frame, json.len(), 0)?;
    frame.extend_from_slice(&json);
    Ok(frame)
}

/// Decode and independently validate one canonical OCDP v1 output frame.
pub fn decode_output_frame(frame: &[u8]) -> Result<ParseOutput, DocumentParserError> {
    if frame.len() > MAX_OUTPUT_FRAME_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let (json_length, body_length) = decode_prelude(frame)?;
    if body_length != 0 || json_length > MAX_OUTPUT_FRAME_BYTES - PRELUDE_BYTES {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    if frame.len() != PRELUDE_BYTES + json_length {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    let json = &frame[PRELUDE_BYTES..];
    let output: ParseOutput =
        serde_json::from_slice(json).map_err(|_| error(DocumentErrorCode::InvalidFrame))?;
    if serde_json::to_vec(&output).map_err(|_| error(DocumentErrorCode::InvalidFrame))? != json {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    validate_output(&output)?;
    Ok(output)
}

fn encode_prelude(
    output: &mut Vec<u8>,
    header_length: usize,
    body_length: usize,
) -> Result<(), DocumentParserError> {
    let header_length = u32::try_from(header_length)
        .map_err(|_| error(DocumentErrorCode::DocumentLimitExceeded))?;
    let body_length =
        u32::try_from(body_length).map_err(|_| error(DocumentErrorCode::DocumentLimitExceeded))?;
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    output.extend_from_slice(&header_length.to_be_bytes());
    output.extend_from_slice(&body_length.to_be_bytes());
    Ok(())
}

fn decode_prelude(frame: &[u8]) -> Result<(usize, usize), DocumentParserError> {
    if frame.len() < PRELUDE_BYTES || &frame[..4] != MAGIC {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    let version = u16::from_be_bytes([frame[4], frame[5]]);
    if version != PROTOCOL_VERSION {
        return Err(error(DocumentErrorCode::InvalidFrame));
    }
    let header_length = u32::from_be_bytes([frame[6], frame[7], frame[8], frame[9]]) as usize;
    let body_length = u32::from_be_bytes([frame[10], frame[11], frame[12], frame[13]]) as usize;
    Ok((header_length, body_length))
}

fn validate_input(header: &InputHeader, body: &[u8]) -> Result<(), DocumentParserError> {
    if header.request_id.is_empty()
        || header.request_id.len() > 128
        || !header.request_id.is_ascii()
        || header
            .request_id
            .bytes()
            .any(|byte| !(0x21..=0x7e).contains(&byte))
        || header.declared_content_type.is_empty()
        || header.declared_content_type.len() > 128
        || !header.declared_content_type.is_ascii()
        || header
            .declared_content_type
            .bytes()
            .any(|byte| byte.is_ascii_whitespace())
        || header.filename.is_empty()
        || header.filename.len() > 255
        || header.filename == "."
        || header.filename == ".."
        || header.filename.contains(['/', '\\'])
        || header.filename.chars().any(char::is_control)
    {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    if !is_lower_hex_sha256(&header.content_sha256) || sha256_hex(body) != header.content_sha256 {
        return Err(error(DocumentErrorCode::ContentDigestMismatch));
    }
    if header.parser_contract_sha256 != PARSER_CONTRACT_SHA256 {
        return Err(error(DocumentErrorCode::ParserContractMismatch));
    }
    if let Some(options) = &header.html_options {
        options.validate()?;
    }
    Ok(())
}

fn validate_output(output: &ParseOutput) -> Result<(), DocumentParserError> {
    match output {
        ParseOutput::Success(success) => {
            if success.version != PROTOCOL_VERSION
                || success.parser_contract_sha256 != PARSER_CONTRACT_SHA256
                || success.detected_content_type != success.format.mime_type()
            {
                return Err(error(DocumentErrorCode::ParserContractMismatch));
            }
            if success.markdown.len() > MAX_MARKDOWN_BYTES
                || has_disallowed_control(&success.markdown)
                || sha256_hex(success.markdown.as_bytes()) != success.markdown_sha256
            {
                return Err(error(DocumentErrorCode::InvalidFrame));
            }
            if success.warnings.len() > 64
                || success.warnings.iter().any(|warning| {
                    warning.is_empty()
                        || warning.len() > 128
                        || !warning
                            .bytes()
                            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
                })
            {
                return Err(error(DocumentErrorCode::InvalidFrame));
            }
            if success.sheet_names.as_ref().is_some_and(|names| {
                names.is_empty()
                    || names.len() > 256
                    || success.sheet_count != u32::try_from(names.len()).ok()
                    || names.iter().any(|name| {
                        name.is_empty() || name.len() > 4096 || has_disallowed_control(name)
                    })
            }) {
                return Err(error(DocumentErrorCode::InvalidFrame));
            }
            validate_metadata(&success.metadata)?;
        }
        ParseOutput::Error(failure) => {
            if failure.version != PROTOCOL_VERSION
                || failure.parser_contract_sha256 != PARSER_CONTRACT_SHA256
            {
                return Err(error(DocumentErrorCode::ParserContractMismatch));
            }
            if failure.error.message != super::stable_message(failure.error.code) {
                return Err(error(DocumentErrorCode::InvalidFrame));
            }
        }
    }
    Ok(())
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
