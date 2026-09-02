use crate::{
    DocumentErrorCode, DocumentFormat, DocumentMetadata, DocumentParserError, MAX_DOCUMENT_BYTES,
    MAX_HEADER_BYTES, MAX_MARKDOWN_BYTES, PARSER_CONTRACT_SHA256, PROTOCOL_VERSION, ParseFailure,
    ParseOutput, ParseRequest, ParseSuccess, admit_document, decode_input_frame,
    encode_output_frame, error, parse_base_url, sha256_hex, validate_metadata,
};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use unicode_normalization::UnicodeNormalization as _;
use xberg::types::metadata::FormatMetadata;
use xberg::{ContentFilterConfig, ExtractInput, ExtractionConfig, OutputFormat, SecurityLimits};

const MAX_CHILD_INPUT_BYTES: usize = 14 + MAX_HEADER_BYTES + MAX_DOCUMENT_BYTES;
const MIN_PDF_TEXT_CHARACTERS: usize = 1;

/// Parse one already decoded, digest-checked OCDP request with the frozen Xberg adapter.
pub async fn parse_document(request: &ParseRequest) -> Result<ParseSuccess, DocumentParserError> {
    let format = admit_document(&request.header, &request.body)?;
    let input = if format == DocumentFormat::Html {
        prepare_html(&request.body, request.header.html_options.as_ref())?
    } else {
        request.body.clone()
    };
    let include_document_furniture = format != DocumentFormat::Html;
    let mut config = ExtractionConfig {
        use_cache: false,
        enable_quality_processing: false,
        disable_ocr: true,
        force_ocr: false,
        output_format: OutputFormat::Markdown,
        security_limits: Some(SecurityLimits {
            max_archive_size: 64 * 1024 * 1024,
            max_compression_ratio: 100,
            max_files_in_archive: 4096,
            max_nesting_depth: 64,
            max_entity_length: 256 * 1024,
            max_content_size: MAX_MARKDOWN_BYTES,
            max_iterations: 2_000_000,
            max_xml_depth: 64,
            max_table_cells: 250_000,
        }),
        content_filter: Some(ContentFilterConfig {
            include_headers: include_document_furniture,
            include_footers: include_document_furniture,
            strip_repeating_text: false,
            include_watermarks: false,
        }),
        max_embedded_file_bytes: Some(0),
        extraction_timeout_secs: None,
        max_concurrent_extractions: Some(1),
        ..ExtractionConfig::default()
    };
    config.images = None;
    config.chunking = None;

    let extraction = xberg::extract(
        ExtractInput::from_bytes(
            input,
            format.mime_type(),
            Some(request.header.filename.clone()),
        ),
        &config,
    )
    .await
    .map_err(|upstream| map_xberg_error(&upstream.to_string()))?;

    if !extraction.errors.is_empty() || extraction.results.len() != 1 {
        let error_text = extraction
            .errors
            .first()
            .map_or("document parse failed", |item| item.message.as_str());
        return Err(map_xberg_error(error_text));
    }
    let document = extraction
        .results
        .into_iter()
        .next()
        .ok_or_else(|| error(DocumentErrorCode::DocumentParseFailed))?;
    let markdown = normalize_markdown(&document.content)?;
    let visible_characters = markdown
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    if format == DocumentFormat::Pdf && visible_characters < MIN_PDF_TEXT_CHARACTERS {
        return Err(error(DocumentErrorCode::DocumentOcrRequired));
    }
    if visible_characters == 0 {
        return Err(error(DocumentErrorCode::DocumentEmpty));
    }

    let (sheet_count, sheet_names) = match document.metadata.format.as_ref() {
        Some(FormatMetadata::Excel(metadata)) => (
            metadata.sheet_count,
            normalize_sheet_names(metadata.sheet_names.as_ref())?,
        ),
        _ => (None, None),
    };
    let metadata = DocumentMetadata {
        title: normalize_metadata_value(document.metadata.title)?,
        authors: normalize_authors(document.metadata.authors)?,
        subject: normalize_metadata_value(document.metadata.subject)?,
        language: normalize_metadata_value(document.metadata.language)?,
    };
    validate_metadata(&metadata)?;

    let page_count = u32::try_from(document.counts.pages)
        .ok()
        .filter(|count| *count > 0);
    let warnings = if document.processing_warnings.is_empty() {
        Vec::new()
    } else {
        vec!["UPSTREAM_WARNING".to_string()]
    };
    let markdown_sha256 = sha256_hex(markdown.as_bytes());
    Ok(ParseSuccess {
        version: PROTOCOL_VERSION,
        format,
        detected_content_type: format.mime_type().to_string(),
        markdown,
        markdown_sha256,
        page_count,
        sheet_count,
        sheet_names,
        metadata,
        warnings,
        parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
    })
}

fn normalize_sheet_names(
    names: Option<&Vec<String>>,
) -> Result<Option<Vec<String>>, DocumentParserError> {
    let Some(names) = names else {
        return Ok(None);
    };
    if names.len() > 256 {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let names = names
        .iter()
        .map(|name| normalize_metadata_value(Some(name.clone())))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok((!names.is_empty()).then_some(names))
}

fn prepare_html(
    body: &[u8],
    options: Option<&crate::HtmlConversionOptions>,
) -> Result<Vec<u8>, DocumentParserError> {
    let source =
        std::str::from_utf8(body).map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))?;
    let dom = tl::parse(source, tl::ParserOptions::default())
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let base = options
        .and_then(|options| options.hostname.as_deref())
        .and_then(parse_base_url)
        .or_else(|| first_document_base(&dom));
    let selected = match options.and_then(|options| options.css_selector.as_deref()) {
        Some(selector) => select_html(&dom, selector)?,
        None => source.to_owned(),
    };
    if selected.len() > MAX_DOCUMENT_BYTES * 2 {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    resolve_relative_links(&selected, base.as_ref()).map(String::into_bytes)
}

fn first_document_base(dom: &tl::VDom<'_>) -> Option<url::Url> {
    let handle = dom.query_selector("base[href]")?.next()?;
    let href = handle
        .get(dom.parser())?
        .as_tag()?
        .attributes()
        .get("href")
        .flatten()?
        .as_utf8_str();
    url::Url::parse(&href).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn select_html(dom: &tl::VDom<'_>, selector: &str) -> Result<String, DocumentParserError> {
    let mut selected = BTreeSet::new();
    for group in selector.split(',').map(str::trim) {
        let matches = dom
            .query_selector(group)
            .ok_or_else(|| error(DocumentErrorCode::InvalidRequest))?;
        for handle in matches {
            selected.insert(handle);
            if selected.len() > 10_000 {
                return Err(error(DocumentErrorCode::DocumentLimitExceeded));
            }
        }
    }
    let mut suppressed = BTreeSet::new();
    let mut output = String::new();
    for handle in &selected {
        if suppressed.contains(handle) {
            continue;
        }
        let node = handle
            .get(dom.parser())
            .ok_or_else(|| error(DocumentErrorCode::DocumentInvalid))?;
        output.push_str(&node.outer_html(dom.parser()));
        if output.len() > MAX_DOCUMENT_BYTES * 2 {
            return Err(error(DocumentErrorCode::DocumentLimitExceeded));
        }
        let mut stack = node
            .children()
            .map_or_else(Vec::new, |children| children.top().to_vec());
        let mut visited = 0_usize;
        while let Some(descendant) = stack.pop() {
            visited = visited.saturating_add(1);
            if visited > 2_000_000 {
                return Err(error(DocumentErrorCode::DocumentLimitExceeded));
            }
            if selected.contains(&descendant) {
                suppressed.insert(descendant);
            }
            if let Some(children) = descendant.get(dom.parser()).and_then(tl::Node::children) {
                stack.extend(children.top().iter().copied());
            }
        }
    }
    Ok(output)
}

fn resolve_relative_links(
    html: &str,
    base: Option<&url::Url>,
) -> Result<String, DocumentParserError> {
    let Some(base) = base else {
        return Ok(html.to_owned());
    };
    let mut dom = tl::parse(html, tl::ParserOptions::default())
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let handles = dom
        .query_selector("[href]")
        .ok_or_else(|| error(DocumentErrorCode::DocumentInvalid))?
        .collect::<Vec<_>>();
    for handle in handles {
        let href = handle
            .get(dom.parser())
            .and_then(tl::Node::as_tag)
            .and_then(|tag| tag.attributes().get("href"))
            .flatten()
            .map(|value| value.as_utf8_str().into_owned());
        let Some(resolved) = href.and_then(|href| base.join(&href).ok()) else {
            continue;
        };
        let tag = handle
            .get_mut(dom.parser_mut())
            .and_then(tl::Node::as_tag_mut)
            .ok_or_else(|| error(DocumentErrorCode::DocumentInvalid))?;
        let value = tl::Bytes::try_from(resolved.to_string())
            .map_err(|_| error(DocumentErrorCode::DocumentLimitExceeded))?;
        tag.attributes_mut().insert("href", Some(value));
    }
    Ok(dom.outer_html())
}

/// Run the complete single-request parser child over stdin/stdout-like streams.
///
/// Protocol failures are returned as a valid OCDP error frame. I/O failures and
/// inability to construct the private current-thread runtime are returned to the
/// caller, which should terminate the child without writing unrelated diagnostics
/// to stdout.
pub fn run_child<R: Read, W: Write>(reader: R, mut writer: W) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(std::io::Error::other)?;
    runtime.block_on(run_child_async(reader, &mut writer))
}

/// Async form of [`run_child`] for an `ocd` entry point that already owns a Tokio runtime.
///
/// The streams remain synchronous because they are the child's dedicated standard
/// input and output. The function performs no filesystem or network I/O.
pub async fn run_child_async<R: Read, W: Write>(reader: R, mut writer: W) -> std::io::Result<()> {
    let mut frame = Vec::new();
    let mut limited = reader.take(u64::try_from(MAX_CHILD_INPUT_BYTES + 1).unwrap_or(u64::MAX));
    limited.read_to_end(&mut frame)?;
    let output = if frame.len() > MAX_CHILD_INPUT_BYTES {
        ParseOutput::Error(ParseFailure::from(error(
            DocumentErrorCode::DocumentLimitExceeded,
        )))
    } else {
        match decode_input_frame(&frame) {
            Ok(request) => match parse_document(&request).await {
                Ok(success) => ParseOutput::Success(success),
                Err(parser_error) => ParseOutput::Error(ParseFailure::from(parser_error)),
            },
            Err(parser_error) => ParseOutput::Error(ParseFailure::from(parser_error)),
        }
    };
    let encoded = encode_output_frame(&output).map_err(std::io::Error::other)?;
    writer.write_all(&encoded)?;
    writer.flush()
}

fn normalize_markdown(input: &str) -> Result<String, DocumentParserError> {
    let mut normalized = String::with_capacity(input.len());
    let mut characters = input.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    let _ = characters.next();
                }
                normalized.push('\n');
            }
            '\n' => normalized.push('\n'),
            '\t' => normalized.push(' '),
            character if character.is_control() => normalized.push(' '),
            character => normalized.push(character),
        }
        if normalized.len() > MAX_MARKDOWN_BYTES {
            return Err(error(DocumentErrorCode::DocumentLimitExceeded));
        }
    }

    let nfc = normalized.nfc().collect::<String>();
    let mut cleaned = String::with_capacity(nfc.len());
    let mut blank_lines = 0_u8;
    for line in nfc.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_lines = blank_lines.saturating_add(1);
            if blank_lines > 2 {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    while cleaned.ends_with("\n\n") {
        let _ = cleaned.pop();
    }
    if cleaned.len() > MAX_MARKDOWN_BYTES {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    Ok(cleaned)
}

fn normalize_metadata_value(value: Option<String>) -> Result<Option<String>, DocumentParserError> {
    value
        .map(|value| {
            let normalized = normalize_markdown(&value)?;
            if normalized.len() > 4096 {
                return Err(error(DocumentErrorCode::DocumentLimitExceeded));
            }
            Ok(normalized.trim().to_string())
        })
        .transpose()
        .map(|value| value.filter(|value| !value.is_empty()))
}

fn normalize_authors(
    authors: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, DocumentParserError> {
    let Some(authors) = authors else {
        return Ok(None);
    };
    if authors.len() > 64 {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    let normalized = authors
        .into_iter()
        .map(|author| normalize_metadata_value(Some(author)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn map_xberg_error(upstream: &str) -> DocumentParserError {
    let lower = upstream.to_ascii_lowercase();
    let code =
        if lower.contains("encrypt") || lower.contains("password") || lower.contains("protected") {
            DocumentErrorCode::DocumentEncrypted
        } else if lower.contains("limit")
            || lower.contains("too large")
            || lower.contains("bomb")
            || lower.contains("too many")
        {
            DocumentErrorCode::DocumentLimitExceeded
        } else if lower.contains("empty") || lower.contains("no content") {
            DocumentErrorCode::DocumentEmpty
        } else {
            DocumentErrorCode::DocumentParseFailed
        };
    error(code)
}
