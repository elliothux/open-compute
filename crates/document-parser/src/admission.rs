use crate::{
    DocumentErrorCode, DocumentFormat, DocumentFormatSpec, FilenameMatcher, InputHeader,
    ProcessingClass, error,
};
use flate2::bufread::GzDecoder;
use std::collections::BTreeSet;
use std::io::{Cursor, Read as _};
use zip::ZipArchive;

const OLE_MAGIC: &[u8; 8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";
const MAX_ZIP_ENTRIES: usize = 4096;
const MAX_ZIP_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ZIP_RATIO: u64 = 100;
const MAX_GZIP_MEMBERS: usize = 16;
const MAX_GZIP_EXPANDED_BYTES: usize = 64 * 1024 * 1024;
const MAX_GZIP_RATIO: usize = 100;

const TEXT_PLAIN: &[&str] = &["text/plain"];
const TOML: &[&str] = &["text/plain", "text/toml", "application/toml"];
const MARKDOWN: &[&str] = &["text/markdown", "text/x-markdown"];
const TEX: &[&str] = &["application/x-tex", "text/x-tex"];
const LATEX: &[&str] = &["application/x-latex", "text/x-latex"];
const SH: &[&str] = &["application/x-sh", "text/x-shellscript"];
const BAT: &[&str] = &["application/x-msdos-batch", "text/x-msdos-batch"];
const PS1: &[&str] = &["text/x-powershell"];
const SGML: &[&str] = &["text/sgml"];
const JSON: &[&str] = &["application/json", "text/json"];
const SQL: &[&str] = &["application/sql", "text/x-sql"];
const YAML: &[&str] = &["application/x-yaml", "text/yaml"];
const CSS: &[&str] = &["text/css"];
const JS: &[&str] = &["application/javascript", "text/javascript"];
const PHP: &[&str] = &["application/x-httpd-php", "text/x-php"];
const PYTHON: &[&str] = &["text/x-python"];
const RUBY: &[&str] = &["text/x-ruby"];
const JAVA: &[&str] = &["text/x-java-source"];
const C: &[&str] = &["text/x-c"];
const CPP: &[&str] = &["text/x-c++"];
const C_HEADER: &[&str] = &["text/x-c-header"];
const GO: &[&str] = &["text/x-go"];
const RUST: &[&str] = &["text/rust", "text/x-rust"];
const SWIFT: &[&str] = &["text/swift", "text/x-swift"];
const DART: &[&str] = &["text/dart", "application/vnd.dart"];
const ELISP: &[&str] = &["application/x-elisp", "text/x-elisp", "text/x-emacs-lisp"];
const GZIP_TEXT: &[&str] = &["text/plain", "application/gzip", "application/x-gzip"];
const PDF: &[&str] = &["application/pdf"];
const JPEG: &[&str] = &["image/jpeg", "image/pjpeg"];
const PNG: &[&str] = &["image/png"];
const WEBP: &[&str] = &["image/webp"];
const SVG: &[&str] = &["image/svg+xml"];
const GIF: &[&str] = &["image/gif"];
const BMP: &[&str] = &["image/bmp", "image/x-bmp"];
const HTML: &[&str] = &["text/html", "application/xhtml+xml"];
const XML: &[&str] = &["application/xml", "text/xml"];
const XLSX: &[&str] = &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"];
const XLSM: &[&str] = &["application/vnd.ms-excel.sheet.macroenabled.12"];
const XLSB: &[&str] = &["application/vnd.ms-excel.sheet.binary.macroenabled.12"];
const XLS: &[&str] = &["application/vnd.ms-excel"];
const ET: &[&str] = &["application/vnd.ms-excel", "application/x-et"];
const DOCX: &[&str] = &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"];
const ODS: &[&str] = &["application/vnd.oasis.opendocument.spreadsheet"];
const ODT: &[&str] = &["application/vnd.oasis.opendocument.text"];
const CSV: &[&str] = &["text/csv", "application/csv"];
const NUMBERS: &[&str] = &["application/vnd.apple.numbers"];

macro_rules! spec {
    ($ext:literal, $mime:expr, $format:ident, $class:ident, $search:literal, $markdown:literal) => {
        DocumentFormatSpec {
            matcher: FilenameMatcher::Suffix($ext),
            extension: $ext,
            mime_types: $mime,
            canonical_mime: $mime[0],
            format: DocumentFormat::$format,
            processing_class: ProcessingClass::$class,
            ai_search: $search,
            markdown_conversion: $markdown,
        }
    };
}

macro_rules! exact {
    ($name:literal, $mime:expr) => {
        DocumentFormatSpec {
            matcher: FilenameMatcher::ExactBasename($name),
            extension: $name,
            mime_types: $mime,
            canonical_mime: $mime[0],
            format: DocumentFormat::Text,
            processing_class: ProcessingClass::PlainText,
            ai_search: true,
            markdown_conversion: false,
        }
    };
}

/// The single authoritative Cloudflare document format registry.
pub const DOCUMENT_FORMATS: &[DocumentFormatSpec] = &[
    spec!("txt", TEXT_PLAIN, Text, PlainText, true, false),
    spec!("rst", TEXT_PLAIN, Text, PlainText, true, false),
    spec!("log", TEXT_PLAIN, Text, PlainText, true, false),
    spec!(
        "log.gz",
        GZIP_TEXT,
        GzipText,
        CompressedPlainText,
        true,
        false
    ),
    spec!("ini", TEXT_PLAIN, Text, PlainText, true, false),
    spec!("conf", TEXT_PLAIN, Text, PlainText, true, false),
    exact!(".env", TEXT_PLAIN),
    spec!("properties", TEXT_PLAIN, Text, PlainText, true, false),
    exact!(".gitignore", TEXT_PLAIN),
    exact!(".editorconfig", TEXT_PLAIN),
    spec!("toml", TOML, Text, PlainText, true, false),
    spec!("markdown", MARKDOWN, Markdown, PlainText, true, false),
    spec!("md", MARKDOWN, Markdown, PlainText, true, false),
    spec!("mdx", MARKDOWN, Markdown, PlainText, true, false),
    spec!("mdoc", MARKDOWN, Markdown, PlainText, true, false),
    spec!("tex", TEX, Text, PlainText, true, false),
    spec!("latex", LATEX, Text, PlainText, true, false),
    spec!("sh", SH, Text, PlainText, true, false),
    spec!("bat", BAT, Text, PlainText, true, false),
    spec!("ps1", PS1, Text, PlainText, true, false),
    spec!("sgml", SGML, Text, PlainText, true, false),
    spec!("json", JSON, Json, PlainText, true, false),
    spec!("sql", SQL, Text, PlainText, true, false),
    spec!("yaml", YAML, Text, PlainText, true, false),
    spec!("yml", YAML, Text, PlainText, true, false),
    spec!("css", CSS, Text, PlainText, true, false),
    spec!("js", JS, Text, PlainText, true, false),
    spec!("php", PHP, Text, PlainText, true, false),
    spec!("py", PYTHON, Text, PlainText, true, false),
    spec!("rb", RUBY, Text, PlainText, true, false),
    spec!("java", JAVA, Text, PlainText, true, false),
    spec!("c", C, Text, PlainText, true, false),
    spec!("cpp", CPP, Text, PlainText, true, false),
    spec!("cxx", CPP, Text, PlainText, true, false),
    spec!("h", C_HEADER, Text, PlainText, true, false),
    spec!("hpp", C_HEADER, Text, PlainText, true, false),
    spec!("go", GO, Text, PlainText, true, false),
    spec!("rs", RUST, Text, PlainText, true, false),
    spec!("swift", SWIFT, Text, PlainText, true, false),
    spec!("dart", DART, Text, PlainText, true, false),
    spec!("el", ELISP, Text, PlainText, true, false),
    spec!("pdf", PDF, Pdf, XbergRich, true, true),
    spec!("jpeg", JPEG, Jpeg, ImageRich, true, true),
    spec!("jpg", JPEG, Jpeg, ImageRich, true, true),
    spec!("png", PNG, Png, ImageRich, true, true),
    spec!("webp", WEBP, Webp, ImageRich, true, true),
    spec!("svg", SVG, Svg, ImageRich, true, true),
    spec!("gif", GIF, Gif, ImageRich, true, true),
    spec!("bmp", BMP, Bmp, ImageRich, true, true),
    spec!("html", HTML, Html, XbergRich, true, true),
    spec!("htm", HTML, Html, XbergRich, true, true),
    spec!("xml", XML, Xml, XbergRich, true, true),
    spec!("xlsx", XLSX, Xlsx, XbergRich, true, true),
    spec!("xlsm", XLSM, Xlsm, XbergRich, true, true),
    spec!("xlsb", XLSB, Xlsb, XbergRich, false, false),
    spec!("xls", XLS, Xls, XbergRich, true, true),
    spec!("et", ET, Et, XbergRich, false, false),
    spec!("docx", DOCX, Docx, XbergRich, true, true),
    spec!("ods", ODS, Ods, XbergRich, true, true),
    spec!("odt", ODT, Odt, XbergRich, true, true),
    spec!("csv", CSV, Csv, XbergRich, true, true),
    spec!("numbers", NUMBERS, Numbers, XbergRich, false, false),
];

/// Return the safely admitted AI Search file entries in deterministic registry order.
#[must_use]
pub fn ai_search_formats() -> Vec<DocumentFormatSpec> {
    DOCUMENT_FORMATS
        .iter()
        .copied()
        .filter(|spec| spec.ai_search)
        .collect()
}

/// Return the safely admitted rich formats exposed by `AI.toMarkdown().supported()`.
#[must_use]
pub fn markdown_conversion_formats() -> Vec<DocumentFormatSpec> {
    DOCUMENT_FORMATS
        .iter()
        .copied()
        .filter(|spec| spec.markdown_conversion)
        .collect()
}

/// Validate size, filename, MIME, magic, and bounded container identity.
pub fn admit_document(
    header: &InputHeader,
    bytes: &[u8],
) -> Result<DocumentFormatSpec, crate::DocumentParserError> {
    if bytes.is_empty() {
        return Err(error(DocumentErrorCode::DocumentEmpty));
    }
    if bytes.len() > usize::try_from(header.max_input_bytes).unwrap_or(usize::MAX) {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let spec = match_filename(&header.filename)?;
    if !spec.ai_search && !spec.markdown_conversion {
        return Err(error(DocumentErrorCode::UnsupportedContentType));
    }
    let declared = canonical_mime(&header.declared_content_type)?;
    if declared != "application/octet-stream"
        && !spec.mime_types.iter().any(|mime| declared == *mime)
    {
        return Err(error(DocumentErrorCode::ContentTypeMismatch));
    }
    if header.html_options.is_some() && spec.format != DocumentFormat::Html {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    validate_content(spec, bytes)?;
    Ok(spec)
}

/// Decode a bounded gzip text document and reject extra non-gzip bytes.
pub fn decode_gzip_text(bytes: &[u8]) -> Result<Vec<u8>, crate::DocumentParserError> {
    let mut remaining = bytes;
    let mut output = Vec::new();
    let mut members = 0_usize;
    while !remaining.is_empty() {
        if !remaining.starts_with(&[0x1f, 0x8b]) || members == MAX_GZIP_MEMBERS {
            return Err(error(DocumentErrorCode::DocumentInvalid));
        }
        let mut decoder = GzDecoder::new(remaining);
        let room = MAX_GZIP_EXPANDED_BYTES.saturating_sub(output.len());
        decoder
            .by_ref()
            .take(u64::try_from(room + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut output)
            .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
        if output.len() > MAX_GZIP_EXPANDED_BYTES {
            return Err(error(DocumentErrorCode::DocumentLimitExceeded));
        }
        let consumed = remaining.len().saturating_sub(decoder.get_ref().len());
        if consumed == 0 || consumed > remaining.len() {
            return Err(error(DocumentErrorCode::DocumentInvalid));
        }
        remaining = &remaining[consumed..];
        members += 1;
    }
    if output.len() > bytes.len().saturating_mul(MAX_GZIP_RATIO) {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    admit_plain(&output)?;
    Ok(output)
}

fn match_filename(filename: &str) -> Result<DocumentFormatSpec, crate::DocumentParserError> {
    validate_filename(filename)?;
    let lower = filename.to_ascii_lowercase();
    DOCUMENT_FORMATS
        .iter()
        .copied()
        .find(|spec| match spec.matcher {
            FilenameMatcher::ExactBasename(name) => lower == name,
            FilenameMatcher::Suffix(extension) => lower
                .strip_suffix(extension)
                .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1),
        })
        .ok_or_else(|| error(DocumentErrorCode::UnsupportedContentType))
}

fn validate_filename(filename: &str) -> Result<(), crate::DocumentParserError> {
    if filename.is_empty()
        || filename.len() > 255
        || filename == "."
        || filename == ".."
        || filename.contains(['/', '\\'])
        || filename.chars().any(char::is_control)
    {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    Ok(())
}

fn canonical_mime(value: &str) -> Result<String, crate::DocumentParserError> {
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let Some((top, subtype)) = mime.split_once('/') else {
        return Err(error(DocumentErrorCode::InvalidRequest));
    };
    if top.is_empty()
        || subtype.is_empty()
        || !mime.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'/' | b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
    {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    Ok(mime)
}

fn validate_content(
    spec: DocumentFormatSpec,
    bytes: &[u8],
) -> Result<(), crate::DocumentParserError> {
    match spec.processing_class {
        ProcessingClass::PlainText => admit_plain(bytes),
        ProcessingClass::CompressedPlainText => decode_gzip_text(bytes).map(|_| ()),
        ProcessingClass::ImageRich => admit_image(spec.format, bytes),
        ProcessingClass::XbergRich => admit_rich(spec.format, bytes),
    }
}

fn admit_plain(bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
        .map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))?;
    if text.chars().any(|character| {
        character == '\0' || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return Err(error(DocumentErrorCode::DocumentInvalid));
    }
    Ok(())
}

fn admit_rich(format: DocumentFormat, bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    match format {
        DocumentFormat::Pdf if bytes.starts_with(b"%PDF-") => admit_pdf(bytes),
        DocumentFormat::Pdf => Err(error(DocumentErrorCode::ContentTypeMismatch)),
        DocumentFormat::Html | DocumentFormat::Xml => {
            admit_plain(bytes)?;
            let text = std::str::from_utf8(bytes)
                .map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))?;
            let trimmed = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
            if trimmed.starts_with('<') && trimmed.contains('>') {
                Ok(())
            } else {
                Err(error(DocumentErrorCode::ContentTypeMismatch))
            }
        }
        DocumentFormat::Csv => admit_plain(bytes),
        DocumentFormat::Xls if bytes.starts_with(OLE_MAGIC) => admit_ole(bytes),
        DocumentFormat::Xls => admit_zip(format, bytes),
        DocumentFormat::Docx
        | DocumentFormat::Xlsx
        | DocumentFormat::Xlsm
        | DocumentFormat::Odt
        | DocumentFormat::Ods => admit_zip(format, bytes),
        _ => Err(error(DocumentErrorCode::ContentTypeMismatch)),
    }
}

fn admit_pdf(bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    let document =
        lopdf::Document::load_mem(bytes).map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    if document.is_encrypted() || document.was_encrypted() {
        Err(error(DocumentErrorCode::DocumentEncrypted))
    } else {
        Ok(())
    }
}

fn admit_ole(bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    let compound = cfb::OpenOptions::new()
        .max_buffer_size(crate::MAX_DOCUMENT_BYTES)
        .open_with(Cursor::new(bytes))
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    if compound.walk().any(|entry| {
        entry.is_stream()
            && (entry.name().eq_ignore_ascii_case("Workbook")
                || entry.name().eq_ignore_ascii_case("Book"))
    }) {
        Ok(())
    } else {
        Err(error(DocumentErrorCode::ContentTypeMismatch))
    }
}

fn admit_zip(expected: DocumentFormat, bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(error(DocumentErrorCode::ContentTypeMismatch));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let mut names = BTreeSet::new();
    let mut expanded = 0_u64;
    let mut odf_mimetype = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
        if entry.encrypted() {
            return Err(error(DocumentErrorCode::DocumentEncrypted));
        }
        let name = entry.name().to_string();
        if name.len() > 1024 || !names.insert(name.clone()) {
            return Err(error(DocumentErrorCode::DocumentInvalid));
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| error(DocumentErrorCode::DocumentLimitExceeded))?;
        if expanded > MAX_ZIP_EXPANDED_BYTES
            || (entry.size() > 0
                && (entry.compressed_size() == 0
                    || entry
                        .compressed_size()
                        .checked_mul(MAX_ZIP_RATIO)
                        .is_none_or(|bound| entry.size() > bound)))
        {
            return Err(error(DocumentErrorCode::DocumentLimitExceeded));
        }
        if name == "mimetype" {
            if entry.size() > 128 {
                return Err(error(DocumentErrorCode::DocumentLimitExceeded));
            }
            let mut value = String::new();
            entry
                .read_to_string(&mut value)
                .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
            odf_mimetype = Some(value);
        }
    }
    let matches = match expected {
        DocumentFormat::Docx => {
            names.contains("[Content_Types].xml") && names.contains("word/document.xml")
        }
        DocumentFormat::Xlsx | DocumentFormat::Xlsm => {
            names.contains("[Content_Types].xml") && names.contains("xl/workbook.xml")
        }
        DocumentFormat::Odt => {
            odf_mimetype.as_deref() == Some("application/vnd.oasis.opendocument.text")
                && names.contains("content.xml")
        }
        DocumentFormat::Ods => {
            odf_mimetype.as_deref() == Some("application/vnd.oasis.opendocument.spreadsheet")
                && names.contains("content.xml")
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(error(DocumentErrorCode::ContentTypeMismatch))
    }
}

fn admit_image(format: DocumentFormat, bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    let magic = match format {
        DocumentFormat::Jpeg => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        DocumentFormat::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        DocumentFormat::Webp => {
            bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
        }
        DocumentFormat::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        DocumentFormat::Bmp => bytes.starts_with(b"BM"),
        DocumentFormat::Svg => return admit_svg(bytes),
        _ => false,
    };
    if !magic {
        return Err(error(DocumentErrorCode::ContentTypeMismatch));
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if width == 0 || height == 0 || width > 8_192 || height > 8_192 || pixels > 16_777_216 {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    Ok(())
}

fn admit_svg(bytes: &[u8]) -> Result<(), crate::DocumentParserError> {
    admit_plain(bytes)?;
    let text =
        std::str::from_utf8(bytes).map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg")
        || lower.contains("<script")
        || lower.contains("<foreignobject")
        || lower.contains("javascript:")
        || has_external_svg_reference(&lower)
    {
        return Err(error(DocumentErrorCode::DocumentInvalid));
    }
    Ok(())
}

fn has_external_svg_reference(svg: &str) -> bool {
    let mut rest = svg;
    while let Some(offset) = rest.find("href=") {
        rest = &rest[offset + 5..];
        let value = rest.trim_start();
        let Some(quote) = value
            .chars()
            .next()
            .filter(|value| matches!(value, '\'' | '"'))
        else {
            return true;
        };
        let value = &value[quote.len_utf8()..];
        let Some(end) = value.find(quote) else {
            return true;
        };
        if !value[..end].starts_with('#') {
            return true;
        }
        rest = &value[end + quote.len_utf8()..];
    }
    let mut rest = svg;
    while let Some(offset) = rest.find("url(") {
        let value = rest[offset + 4..].trim_start_matches([' ', '\'', '"']);
        if !value.starts_with('#') {
            return true;
        }
        rest = &value[1..];
    }
    false
}
