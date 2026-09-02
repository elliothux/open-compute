use crate::{
    DocumentErrorCode, DocumentFormat, DocumentParserError, InputHeader, MAX_DOCUMENT_BYTES,
    SupportedFormat, error,
};
use std::collections::BTreeSet;
use std::io::{Cursor, Read as _};
use zip::ZipArchive;

const OLE_MAGIC: &[u8; 8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";
const MAX_ZIP_ENTRIES: usize = 4096;
const MAX_ZIP_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ZIP_RATIO: u64 = 100;

const SUPPORTED: [DocumentFormat; 13] = [
    DocumentFormat::Csv,
    DocumentFormat::Docx,
    DocumentFormat::Html,
    DocumentFormat::Json,
    DocumentFormat::Markdown,
    DocumentFormat::Ods,
    DocumentFormat::Odt,
    DocumentFormat::Pdf,
    DocumentFormat::Text,
    DocumentFormat::Xls,
    DocumentFormat::Xlsm,
    DocumentFormat::Xlsx,
    DocumentFormat::Xml,
];

/// Return the closed, deterministic format list implemented by this parser contract.
#[must_use]
pub fn supported_formats() -> Vec<SupportedFormat> {
    SUPPORTED
        .iter()
        .map(|format| SupportedFormat {
            extension: format.extension(),
            mime_type: format.mime_type(),
        })
        .collect()
}

/// Validate size, extension, MIME, magic, and bounded container identity.
pub fn admit_document(
    header: &InputHeader,
    bytes: &[u8],
) -> Result<DocumentFormat, DocumentParserError> {
    if bytes.is_empty() {
        return Err(error(DocumentErrorCode::DocumentEmpty));
    }
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let extension = extension(&header.filename)?;
    let format = SUPPORTED
        .iter()
        .copied()
        .find(|format| format.extension() == extension)
        .ok_or_else(|| error(DocumentErrorCode::UnsupportedContentType))?;
    if !header
        .declared_content_type
        .eq_ignore_ascii_case(format.mime_type())
    {
        return Err(error(DocumentErrorCode::ContentTypeMismatch));
    }
    if header.html_options.is_some() && format != DocumentFormat::Html {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }

    match format {
        DocumentFormat::Text
        | DocumentFormat::Markdown
        | DocumentFormat::Html
        | DocumentFormat::Xml
        | DocumentFormat::Json
        | DocumentFormat::Csv => admit_utf8(format, bytes).map(|()| format),
        DocumentFormat::Pdf if bytes.starts_with(b"%PDF-") => admit_pdf(bytes).map(|()| format),
        DocumentFormat::Pdf => Err(error(DocumentErrorCode::ContentTypeMismatch)),
        DocumentFormat::Xls if bytes.starts_with(OLE_MAGIC) => admit_ole(bytes).map(|()| format),
        DocumentFormat::Xls => Err(error(DocumentErrorCode::ContentTypeMismatch)),
        _ if bytes.starts_with(b"PK\x03\x04") => admit_zip(format, bytes).map(|()| format),
        _ => Err(error(DocumentErrorCode::ContentTypeMismatch)),
    }
}

fn admit_pdf(bytes: &[u8]) -> Result<(), DocumentParserError> {
    let document =
        lopdf::Document::load_mem(bytes).map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    if document.is_encrypted() || document.was_encrypted() {
        Err(error(DocumentErrorCode::DocumentEncrypted))
    } else {
        Ok(())
    }
}

fn extension(filename: &str) -> Result<String, DocumentParserError> {
    if filename.is_empty()
        || filename.len() > 255
        || filename == "."
        || filename == ".."
        || filename.contains(['/', '\\'])
        || filename.chars().any(char::is_control)
    {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    let Some((stem, extension)) = filename.rsplit_once('.') else {
        return Err(error(DocumentErrorCode::UnsupportedContentType));
    };
    if stem.is_empty() || extension.is_empty() || !extension.is_ascii() {
        return Err(error(DocumentErrorCode::InvalidRequest));
    }
    Ok(extension.to_ascii_lowercase())
}

fn admit_ole(bytes: &[u8]) -> Result<(), DocumentParserError> {
    let compound = cfb::OpenOptions::new()
        .max_buffer_size(MAX_DOCUMENT_BYTES)
        .open_with(Cursor::new(bytes))
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let has_workbook = compound.walk().any(|entry| {
        entry.is_stream()
            && (entry.name().eq_ignore_ascii_case("Workbook")
                || entry.name().eq_ignore_ascii_case("Book"))
    });
    if has_workbook {
        Ok(())
    } else {
        Err(error(DocumentErrorCode::ContentTypeMismatch))
    }
}

fn admit_zip(expected: DocumentFormat, bytes: &[u8]) -> Result<(), DocumentParserError> {
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
        DocumentFormat::Pdf
        | DocumentFormat::Xls
        | DocumentFormat::Text
        | DocumentFormat::Markdown
        | DocumentFormat::Html
        | DocumentFormat::Xml
        | DocumentFormat::Json
        | DocumentFormat::Csv => false,
    };
    if matches {
        Ok(())
    } else {
        Err(error(DocumentErrorCode::ContentTypeMismatch))
    }
}

fn admit_utf8(format: DocumentFormat, bytes: &[u8]) -> Result<(), DocumentParserError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))?;
    if text.contains('\0') {
        return Err(error(DocumentErrorCode::DocumentInvalid));
    }
    let trimmed = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    let plausible = match format {
        DocumentFormat::Html => trimmed.starts_with('<') && trimmed.contains('>'),
        DocumentFormat::Xml => trimmed.starts_with('<') && trimmed.contains('>'),
        DocumentFormat::Json => matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'[')),
        DocumentFormat::Csv => trimmed.contains(',') || trimmed.contains('\n'),
        DocumentFormat::Text | DocumentFormat::Markdown => true,
        _ => false,
    };
    if plausible {
        Ok(())
    } else {
        Err(error(DocumentErrorCode::ContentTypeMismatch))
    }
}
