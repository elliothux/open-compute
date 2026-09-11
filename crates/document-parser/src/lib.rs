//! Bounded Xberg document parsing behind the private OCDP v1 child protocol.
//!
//! This crate accepts only in-memory bytes. It owns neither filesystem paths,
//! network access, tenant authority, chunking, nor embedding. The service crate
//! is responsible for running it in the short-lived parser child and enforcing
//! process-level CPU, memory, and wall-clock limits.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod admission;
mod frame;
mod parser;
mod tessdata;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub use admission::{
    DOCUMENT_FORMATS, admit_document, ai_search_formats, canonical_content_type, decode_gzip_text,
    markdown_conversion_formats,
};
pub use frame::{decode_input_frame, decode_output_frame, encode_input_frame, encode_output_frame};
pub use parser::{parse_document, run_child, run_child_async};
pub use tessdata::{
    OCR_LANGUAGE_EXPRESSION, TESSDATA_CONTRACT_SHA256, materialize_tessdata, verify_tessdata_dir,
};

/// OCDP wire protocol revision.
pub const PROTOCOL_VERSION: u16 = 1;
/// Maximum accepted document body size, matching AI Search's public item limit.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
/// Maximum canonical JSON input header size.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Maximum normalized Markdown size returned by the parser.
pub const MAX_MARKDOWN_BYTES: usize = 16 * 1024 * 1024;
/// Maximum complete framed response size accepted from a parser child.
pub const MAX_OUTPUT_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// Canonical parser-contract manifest hashed by [`PARSER_CONTRACT_SHA256`].
pub const PARSER_CONTRACT_MANIFEST: &str = "ocdp=1\n\
xberg=1.1.5\n\
features=excel,ocr,office,pdf,svg,tokio-runtime,xml\n\
formats=cloudflare-candidates-62,ai-search-advertised-59,markdown-conversion-advertised-18\n\
ocr=tesseract-5.5.3,eng+chi_sim+chi_tra,no-osd\n\
ocr_adapter=xberg-language-validator-aliases-zho+chinese_cht-v1\n\
tessdata_revision=87416418657359cb625c412a48b6e1d6d41c29bd\n\
image_preprocessing=lanczos3,jpeg-q90,white-background,pdf-page-fit,v2\n\
vision_candidates=max16,encoded_bytes1048576\n\
html_options=v1\n\
max_document_bytes=67108864\n\
max_header_bytes=16384\n\
max_markdown_bytes=16777216\n\
max_output_frame_bytes=67108864\n\
zip_entries=4096\n\
zip_expanded_bytes=67108864\n\
zip_ratio=100\n\
security=max_nesting_depth:64,max_entity_length:262144,max_iterations:2000000,max_xml_depth:64,max_table_cells:250000\n\
normalizer=v1\n\
output_metadata=page_count,sheet_count,sheet_names\n";
/// Hash of the Xberg pin, features, adapter revision, format set, and limits.
pub const PARSER_CONTRACT_SHA256: &str =
    "2e936ac25baaf92ff88262463054e494d437a61fa19b5c465d78701824c6f197";

/// A public-admission document format implemented by the parser child.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    /// UTF-8 plain text.
    Text,
    /// UTF-8 Markdown.
    Markdown,
    /// UTF-8 HTML.
    Html,
    /// UTF-8 XML.
    Xml,
    /// UTF-8 JSON.
    Json,
    /// UTF-8 comma-separated values.
    Csv,
    /// Bounded gzip-compressed UTF-8 log text.
    GzipText,
    /// Text-layer PDF.
    Pdf,
    /// Word OOXML.
    Docx,
    /// Excel OOXML.
    Xlsx,
    /// Macro-enabled Excel OOXML. Macros are never executed or returned.
    Xlsm,
    /// Binary Excel workbook.
    Xlsb,
    /// Legacy OLE/BIFF Excel workbook.
    Xls,
    /// Kingsoft spreadsheet with container-directed extraction.
    Et,
    /// `OpenDocument` text.
    Odt,
    /// `OpenDocument` spreadsheet.
    Ods,
    /// Apple Numbers spreadsheet.
    Numbers,
    /// JPEG image.
    Jpeg,
    /// PNG image.
    Png,
    /// WebP image.
    Webp,
    /// Sanitized SVG image.
    Svg,
    /// First-frame GIF image.
    Gif,
    /// BMP image.
    Bmp,
}

impl DocumentFormat {
    /// Canonical lowercase extension without a leading dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Html => "html",
            Self::Xml => "xml",
            Self::Json => "json",
            Self::Csv => "csv",
            Self::GzipText => "log.gz",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Xlsm => "xlsm",
            Self::Xlsb => "xlsb",
            Self::Xls => "xls",
            Self::Et => "et",
            Self::Odt => "odt",
            Self::Ods => "ods",
            Self::Numbers => "numbers",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Svg => "svg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
        }
    }

    /// Canonical detected MIME emitted by the child.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Text => "text/plain",
            Self::Markdown => "text/markdown",
            Self::Html => "text/html",
            Self::Xml => "application/xml",
            Self::Json => "application/json",
            Self::Csv => "text/csv",
            Self::GzipText => "text/plain",
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Xlsm => "application/vnd.ms-excel.sheet.macroenabled.12",
            Self::Xlsb => "application/vnd.ms-excel.sheet.binary.macroenabled.12",
            Self::Xls => "application/vnd.ms-excel",
            Self::Et => "application/vnd.ms-excel",
            Self::Odt => "application/vnd.oasis.opendocument.text",
            Self::Ods => "application/vnd.oasis.opendocument.spreadsheet",
            Self::Numbers => "application/vnd.apple.numbers",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
        }
    }
}

/// Filename matching mode for one registry item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilenameMatcher {
    /// Case-insensitive suffix, including compound suffixes such as `log.gz`.
    Suffix(&'static str),
    /// Case-insensitive exact basename such as `.env`.
    ExactBasename(&'static str),
}

/// Parser path selected by a registry item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessingClass {
    /// BOM-aware validated UTF-8 normalized without Xberg.
    PlainText,
    /// Bounded gzip expanded before plain-text normalization.
    CompressedPlainText,
    /// Rich document processed by the fixed Xberg adapter.
    XbergRich,
    /// Image decoded, OCR'd, and optionally returned as a vision candidate.
    ImageRich,
}

/// One authoritative document-format registry item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentFormatSpec {
    /// Filename matcher.
    pub matcher: FilenameMatcher,
    /// Cloudflare extension without the leading dot, or exact dotfile basename.
    pub extension: &'static str,
    /// Accepted normalized MIME aliases.
    pub mime_types: &'static [&'static str],
    /// Canonical MIME returned to callers.
    pub canonical_mime: &'static str,
    /// Internal parser format.
    pub format: DocumentFormat,
    /// Processing class.
    pub processing_class: ProcessingClass,
    /// Whether AI Search accepts this item.
    pub ai_search: bool,
    /// Whether Markdown Conversion advertises this item.
    pub markdown_conversion: bool,
}

/// Canonical OCDP v1 request header.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputHeader {
    /// Opaque local correlation identifier; it is not tenant identity.
    pub request_id: String,
    /// Validated logical filename, never a filesystem path.
    pub filename: String,
    /// Caller-declared canonical content type.
    pub declared_content_type: String,
    /// Lowercase SHA-256 of the body bytes.
    pub content_sha256: String,
    /// Exact parser contract expected by the parent.
    pub parser_contract_sha256: String,
    /// Parent-resolved source byte limit, bounded by the protocol hard cap.
    pub max_input_bytes: u64,
    /// Parent-verified absolute tessdata directory, required only when OCR runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tessdata_path: Option<String>,
    /// Maximum normalized image candidates requested by the parent; zero disables them.
    pub vision_candidate_limit: u16,
    /// Bounded Cloudflare HTML conversion options, interpreted only for HTML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_options: Option<HtmlConversionOptions>,
}

/// HTML conversion options carried only across the private parser-child wire.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HtmlConversionOptions {
    /// Optional HTTP(S) base used to resolve relative links without fetching it.
    pub hostname: Option<String>,
    /// Optional bounded selector whose matching elements are retained.
    pub css_selector: Option<String>,
}

impl HtmlConversionOptions {
    /// Validate the closed, no-network HTML option contract.
    pub fn validate(&self) -> Result<(), DocumentParserError> {
        if let Some(hostname) = &self.hostname
            && (hostname.is_empty()
                || hostname.len() > 2048
                || hostname.chars().any(char::is_control)
                || parse_base_url(hostname).is_none())
        {
            return Err(error(DocumentErrorCode::InvalidRequest));
        }
        if let Some(selector) = &self.css_selector {
            let groups = selector.split(',').map(str::trim).collect::<Vec<_>>();
            if selector.is_empty()
                || selector.len() > 512
                || selector.chars().any(char::is_control)
                || selector.contains(['{', '}', '@', '\\'])
                || groups.is_empty()
                || groups.len() > 16
                || groups
                    .iter()
                    .any(|group| group.is_empty() || tl::parse_query_selector(group).is_none())
            {
                return Err(error(DocumentErrorCode::InvalidRequest));
            }
        }
        Ok(())
    }
}

/// Decoded OCDP input passed to the in-child parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseRequest {
    /// Validated request header.
    pub header: InputHeader,
    /// Bounded raw document bytes.
    pub body: Vec<u8>,
}

/// Closed metadata allowlist returned by the parser.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentMetadata {
    /// Document title, when present and bounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Document authors, when present and bounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    /// Document subject, when present and bounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Document language, when present and bounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Semantic interpretation of the normalized document content.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsedContentKind {
    /// Validated source text whose markup is not interpreted by the parser.
    PlainText,
    /// Normalized Markdown produced from a rich document or image.
    Markdown,
}

/// Bounded, metadata-free raster returned only across the private parser frame.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisionCandidate {
    /// Base64-encoded deterministic JPEG bytes.
    pub data_base64: String,
    /// Encoded MIME type, always `image/jpeg`.
    pub mime_type: String,
    /// Raster width.
    pub width: u32,
    /// Raster height.
    pub height: u32,
    /// SHA-256 of decoded JPEG bytes.
    pub sha256: String,
    /// One-indexed source PDF page, or absent for a standalone image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_page: Option<u32>,
    /// Whether Tesseract ran for the source.
    pub ocr_performed: bool,
    /// Mean OCR confidence when supplied by the engine.
    pub ocr_confidence_milli: Option<u32>,
}

/// Successful normalized parser output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParseSuccess {
    /// Output schema version, always one.
    pub version: u16,
    /// Admitted document format.
    pub format: DocumentFormat,
    /// Canonical MIME determined by admission.
    pub detected_content_type: String,
    /// Whether `content` preserves plain-text semantics or is normalized Markdown.
    pub content_kind: ParsedContentKind,
    /// LF-only, NFC-normalized Markdown without disallowed controls.
    pub markdown: String,
    /// Lowercase SHA-256 of `markdown` bytes.
    pub markdown_sha256: String,
    /// PDF page count when known.
    pub page_count: Option<u32>,
    /// Spreadsheet sheet count when known.
    pub sheet_count: Option<u32>,
    /// Spreadsheet sheet names in workbook order when known.
    pub sheet_names: Option<Vec<String>>,
    /// Closed, bounded metadata allowlist.
    pub metadata: DocumentMetadata,
    /// Stable warning codes; upstream diagnostic text is never returned.
    pub warnings: Vec<String>,
    /// Normalized images for parent-owned VLM requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vision_candidates: Vec<VisionCandidate>,
    /// Exact parser contract used to produce this output.
    pub parser_contract_sha256: String,
}

/// Stable document-parser error codes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DocumentErrorCode {
    /// OCDP magic, version, lengths, canonical JSON, or trailing bytes are invalid.
    InvalidFrame,
    /// Input header or logical filename is invalid.
    InvalidRequest,
    /// Body or parser output exceeds a hard limit.
    DocumentLimitExceeded,
    /// Body digest does not match the header.
    ContentDigestMismatch,
    /// Parent and child parser contracts differ.
    ParserContractMismatch,
    /// Extension or MIME is outside the closed admission set.
    UnsupportedContentType,
    /// Extension, MIME, magic, or container identity disagree.
    ContentTypeMismatch,
    /// Container or document bytes are malformed or fail bounded admission.
    DocumentInvalid,
    /// Document is encrypted or password protected.
    DocumentEncrypted,
    /// Document has no indexable content.
    DocumentEmpty,
    /// Fixed OCR assets or engine are unavailable or failed integrity validation.
    DocumentOcrUnavailable,
    /// A normalized image cannot fit the configured VLM input envelope.
    DocumentVisionInputTooLarge,
    /// Xberg could not parse the admitted document.
    DocumentParseFailed,
}

impl DocumentErrorCode {
    /// Stable uppercase string form used across the child boundary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidFrame => "INVALID_FRAME",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::DocumentLimitExceeded => "DOCUMENT_LIMIT_EXCEEDED",
            Self::ContentDigestMismatch => "CONTENT_DIGEST_MISMATCH",
            Self::ParserContractMismatch => "PARSER_CONTRACT_MISMATCH",
            Self::UnsupportedContentType => "UNSUPPORTED_CONTENT_TYPE",
            Self::ContentTypeMismatch => "CONTENT_TYPE_MISMATCH",
            Self::DocumentInvalid => "DOCUMENT_INVALID",
            Self::DocumentEncrypted => "DOCUMENT_ENCRYPTED",
            Self::DocumentEmpty => "DOCUMENT_EMPTY",
            Self::DocumentOcrUnavailable => "DOCUMENT_OCR_UNAVAILABLE",
            Self::DocumentVisionInputTooLarge => "DOCUMENT_VISION_INPUT_TOO_LARGE",
            Self::DocumentParseFailed => "DOCUMENT_PARSE_FAILED",
        }
    }
}

/// Sanitized parser error safe to serialize across the private child boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentParserError {
    /// Stable machine-readable code.
    pub code: DocumentErrorCode,
    /// Stable content-free description.
    pub message: String,
}

impl DocumentParserError {
    pub(crate) fn new(code: DocumentErrorCode) -> Self {
        Self {
            code,
            message: stable_message(code).to_string(),
        }
    }
}

impl fmt::Display for DocumentParserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for DocumentParserError {}

/// One closed OCDP output payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ParseOutput {
    /// Successful parse.
    Success(Box<ParseSuccess>),
    /// Structured parse or protocol failure.
    Error(ParseFailure),
}

/// OCDP structured error payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParseFailure {
    /// Output schema version, always one.
    pub version: u16,
    /// Sanitized stable error.
    pub error: DocumentParserError,
    /// Child parser contract, allowing the parent to reject stale binaries.
    pub parser_contract_sha256: String,
}

impl From<DocumentParserError> for ParseFailure {
    fn from(error: DocumentParserError) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            error,
            parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
        }
    }
}

pub(crate) fn error(code: DocumentErrorCode) -> DocumentParserError {
    DocumentParserError::new(code)
}

const fn stable_message(code: DocumentErrorCode) -> &'static str {
    match code {
        DocumentErrorCode::InvalidFrame => "the parser frame is invalid",
        DocumentErrorCode::InvalidRequest => "the parser request is invalid",
        DocumentErrorCode::DocumentLimitExceeded => "a document parser limit was exceeded",
        DocumentErrorCode::ContentDigestMismatch => "the document digest does not match",
        DocumentErrorCode::ParserContractMismatch => "the parser contract does not match",
        DocumentErrorCode::UnsupportedContentType => "the document type is unsupported",
        DocumentErrorCode::ContentTypeMismatch => "the document type does not match its content",
        DocumentErrorCode::DocumentInvalid => "the document container is invalid",
        DocumentErrorCode::DocumentEncrypted => "the document is encrypted",
        DocumentErrorCode::DocumentEmpty => "the document contains no indexable text",
        DocumentErrorCode::DocumentOcrUnavailable => "document OCR is unavailable",
        DocumentErrorCode::DocumentVisionInputTooLarge => {
            "the normalized image exceeds the vision input limit"
        }
        DocumentErrorCode::DocumentParseFailed => "the document could not be parsed",
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub(crate) fn parse_base_url(value: &str) -> Option<url::Url> {
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    url::Url::parse(&candidate).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

pub(crate) fn validate_metadata(metadata: &DocumentMetadata) -> Result<(), DocumentParserError> {
    let mut values = BTreeMap::new();
    if let Some(title) = &metadata.title {
        values.insert("title", title.as_str());
    }
    if let Some(subject) = &metadata.subject {
        values.insert("subject", subject.as_str());
    }
    if let Some(language) = &metadata.language {
        values.insert("language", language.as_str());
    }
    if values
        .values()
        .any(|value| value.len() > 4096 || has_disallowed_control(value))
        || metadata.authors.as_ref().is_some_and(|authors| {
            authors.len() > 64
                || authors
                    .iter()
                    .any(|author| author.len() > 1024 || has_disallowed_control(author))
        })
    {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    Ok(())
}

pub(crate) fn has_disallowed_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() && character != '\n')
}

#[cfg(test)]
mod tests;
