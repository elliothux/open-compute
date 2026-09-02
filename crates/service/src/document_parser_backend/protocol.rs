//! Markdown Conversion wire schema, validation, rendering, and stable error mapping.

use super::{ERROR_HEADER, MAX_MIME_BYTES, MAX_NAME_BYTES};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{AccountId, DeploymentId, ErrorCode, PlatformError};
use open_compute_document_parser::{DocumentErrorCode, DocumentMetadata, HtmlConversionOptions};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug)]
pub(super) struct ParserAuthority {
    pub(super) account: AccountId,
    pub(super) deployment: DeploymentId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TransformRequest {
    pub(super) schema_version: u32,
    pub(super) files: Vec<WireDocument>,
    #[serde(default)]
    pub(super) options: ConversionOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WireDocument {
    pub(super) name: String,
    pub(super) mime_type: String,
    pub(super) data_base64: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConversionOptions {
    pub(super) output: Option<OutputOptions>,
    pub(super) html: Option<HtmlOptions>,
    pub(super) pdf: Option<PdfOptions>,
}

impl ConversionOptions {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if let Some(html) = &self.html {
            HtmlConversionOptions {
                hostname: html.hostname.clone(),
                css_selector: html.css_selector.clone(),
            }
            .validate()
            .map_err(|_| option_unsupported())?;
        }
        Ok(())
    }

    pub(super) fn html_options(&self, declared_mime: &str) -> Option<HtmlConversionOptions> {
        if declared_mime == "text/html" {
            self.html.as_ref().map(|html| HtmlConversionOptions {
                hostname: html.hostname.clone(),
                css_selector: html.css_selector.clone(),
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum OutputFormat {
    Markdown,
    Text,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OutputOptions {
    pub(super) format: Option<OutputFormat>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HtmlOptions {
    pub(super) hostname: Option<String>,
    pub(super) css_selector: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PdfOptions {
    pub(super) metadata: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResponseEnvelope<T> {
    pub(super) schema_version: u32,
    pub(super) result: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SupportedResponse {
    pub(super) extension: String,
    pub(super) mime_type: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum ConversionResponse {
    Success {
        id: String,
        name: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        format: OutputFormat,
        tokens: u64,
        data: String,
    },
    Error {
        id: String,
        name: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        format: ErrorFormat,
        error: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ErrorFormat {
    Error,
}

pub(super) fn markdown_with_metadata(metadata: &DocumentMetadata, markdown: &str) -> String {
    let mut fields = Vec::new();
    if let Some(title) = &metadata.title {
        fields.push(format!("title: {}", yaml_scalar(title)));
    }
    if let Some(authors) = &metadata.authors
        && !authors.is_empty()
    {
        fields.push(format!("authors: {}", yaml_scalar(&authors.join(", "))));
    }
    if let Some(subject) = &metadata.subject {
        fields.push(format!("subject: {}", yaml_scalar(subject)));
    }
    if let Some(language) = &metadata.language {
        fields.push(format!("language: {}", yaml_scalar(language)));
    }
    if fields.is_empty() {
        return markdown.to_owned();
    }
    format!("---\n{}\n---\n\n{markdown}", fields.join("\n"))
}

pub(super) fn yaml_scalar(value: &str) -> String {
    format!("{value:?}")
}

pub(super) fn markdown_to_text(markdown: &str) -> String {
    let mut output = String::with_capacity(markdown.len());
    let mut code_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            code_fence = !code_fence;
            continue;
        }
        let mut line = if code_fence {
            trimmed
        } else {
            trimmed
                .trim_start_matches('#')
                .trim_start_matches(['-', '*', '+', '>'])
                .trim_start()
        }
        .replace(['`', '*', '_'], "");
        if line.starts_with('[')
            && let Some(close) = line.find("](")
            && line.ends_with(')')
        {
            line = line[1..close].to_owned();
        }
        if !line.is_empty() {
            output.push_str(&line);
            output.push('\n');
        }
    }
    output.trim_end().to_owned()
}

pub(super) fn estimate_tokens(text: &str) -> u64 {
    let mut tokens = 0_u64;
    let mut word_len = 0_u64;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            word_len = word_len.saturating_add(1);
        } else {
            if word_len > 0 {
                tokens = tokens.saturating_add(word_len.div_ceil(4));
                word_len = 0;
            }
            if !character.is_whitespace() {
                tokens = tokens.saturating_add(1);
            }
        }
    }
    if word_len > 0 {
        tokens = tokens.saturating_add(word_len.div_ceil(4));
    }
    tokens
}

pub(super) fn validate_logical_name(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > MAX_NAME_BYTES
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(input());
    }
    Ok(())
}

pub(super) fn validate_mime(value: &str) -> Result<(), PlatformError> {
    let Some((top, subtype)) = value.split_once('/') else {
        return Err(input());
    };
    let token = |byte: u8| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
            )
    };
    if value.len() > MAX_MIME_BYTES
        || top.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !top.bytes().all(token)
        || !subtype.bytes().all(token)
    {
        return Err(input());
    }
    Ok(())
}

pub(super) fn map_parser_protocol(
    _: open_compute_document_parser::DocumentParserError,
) -> ErrorCode {
    ErrorCode::DocumentProtocolError
}

pub(super) fn map_document_code(code: DocumentErrorCode) -> ErrorCode {
    match code {
        DocumentErrorCode::InvalidFrame
        | DocumentErrorCode::ContentDigestMismatch
        | DocumentErrorCode::ParserContractMismatch => ErrorCode::DocumentProtocolError,
        DocumentErrorCode::InvalidRequest
        | DocumentErrorCode::ContentTypeMismatch
        | DocumentErrorCode::DocumentInvalid => ErrorCode::DocumentInputInvalid,
        DocumentErrorCode::DocumentLimitExceeded => ErrorCode::DocumentLimitExceeded,
        DocumentErrorCode::UnsupportedContentType => ErrorCode::DocumentFormatUnsupported,
        DocumentErrorCode::DocumentEncrypted => ErrorCode::DocumentEncrypted,
        DocumentErrorCode::DocumentEmpty => ErrorCode::DocumentEmpty,
        DocumentErrorCode::DocumentOcrRequired => ErrorCode::DocumentOcrRequired,
        DocumentErrorCode::DocumentParseFailed => ErrorCode::DocumentParseFailed,
    }
}

pub(super) fn parse_header<T: FromStr>(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<T, PlatformError> {
    text_header(headers, name)?.parse().map_err(|_| protocol())
}

pub(super) fn text_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, PlatformError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(protocol)
}

pub(super) fn json_response(value: &impl Serialize) -> Result<Response, PlatformError> {
    let body = serde_json::to_vec(value).map_err(|_| protocol())?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .map_err(|_| protocol())
}

pub(super) fn document_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::DocumentLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::DocumentTimeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::DocumentUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(error.code().as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

pub(super) fn protocol() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentProtocolError,
        "Markdown Conversion protocol is invalid",
    )
}

pub(super) fn input() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentInputInvalid,
        "Markdown Conversion input is invalid",
    )
}

pub(super) fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentLimitExceeded,
        "Markdown Conversion limit was exceeded",
    )
}

pub(super) fn timeout() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentTimeout,
        "Markdown Conversion request timed out",
    )
}

pub(super) fn option_unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentOptionUnsupported,
        "Markdown Conversion option is unsupported",
    )
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::DocumentUnavailable,
        "document parser is unavailable",
    )
}
