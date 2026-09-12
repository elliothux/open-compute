use crate::{
    DocumentErrorCode, DocumentFormat, DocumentMetadata, DocumentParserError, MAX_DOCUMENT_BYTES,
    MAX_HEADER_BYTES, MAX_MARKDOWN_BYTES, PARSER_CONTRACT_SHA256, PROTOCOL_VERSION, ParseFailure,
    ParseOutput, ParseRequest, ParseSuccess, ParsedContentKind, ProcessingClass, VisionCandidate,
    admit_document, decode_gzip_text, decode_input_frame, encode_output_frame, error,
    parse_base_url, sha256_hex, validate_metadata,
};
use base64::Engine as _;
use image::{DynamicImage, GenericImageView as _, ImageDecoder as _, ImageReader};
use std::collections::BTreeSet;
use std::future::Future;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::pin::Pin;
use unicode_normalization::UnicodeNormalization as _;
use xberg::types::metadata::FormatMetadata;
use xberg::{
    ContentFilterConfig, ExtractInput, ExtractionConfig, OcrConfig, OutputFormat, SecurityLimits,
};

const MAX_CHILD_INPUT_BYTES: usize = 14 + MAX_HEADER_BYTES + MAX_DOCUMENT_BYTES;
const MAX_IMAGE_PIXELS: u64 = 16_777_216;
const VISION_MAX_WIDTH: u32 = 1_280;
const VISION_MAX_HEIGHT: u32 = 720;
const VISION_MAX_PIXELS: u64 = 921_600;
const VISION_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Parse one already decoded, digest-checked OCDP request with the frozen Xberg adapter.
pub fn parse_document(
    request: &ParseRequest,
) -> Pin<Box<dyn Future<Output = Result<ParseSuccess, DocumentParserError>> + Send + '_>> {
    Box::pin(parse_document_inner(request))
}

async fn parse_document_inner(request: &ParseRequest) -> Result<ParseSuccess, DocumentParserError> {
    let spec = admit_document(&request.header, &request.body)?;
    match spec.processing_class {
        ProcessingClass::PlainText => {
            return plain_text_success(&spec, &decode_plain_text(&request.body)?);
        }
        ProcessingClass::CompressedPlainText => {
            let expanded = decode_gzip_text(&request.body)?;
            return plain_text_success(&spec, &decode_plain_text(&expanded)?);
        }
        ProcessingClass::XbergRich | ProcessingClass::ImageRich => {}
    }
    let input = if spec.format == DocumentFormat::Html {
        prepare_html(&request.body, request.header.html_options.as_ref())?
    } else {
        request.body.clone()
    };
    let tessdata_path = if matches!(spec.format, DocumentFormat::Pdf)
        || spec.processing_class == ProcessingClass::ImageRich
    {
        Some(required_tessdata_path(request)?)
    } else {
        None
    };
    let mut vision_candidates = if spec.processing_class == ProcessingClass::ImageRich
        && request.header.vision_candidate_limit > 0
    {
        vec![image_candidate(spec.format, &request.body, None)?]
    } else {
        Vec::new()
    };
    let config = extraction_config(spec.format != DocumentFormat::Html, tessdata_path);
    let extraction = xberg::extract(
        ExtractInput::from_bytes(
            input,
            spec.canonical_mime,
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
    if spec.format == DocumentFormat::Pdf
        && request.header.vision_candidate_limit > 0
        && document.extraction_method == Some(xberg::types::ExtractionMethod::Ocr)
    {
        vision_candidates = pdf_vision_candidates(
            &request.body,
            usize::from(request.header.vision_candidate_limit),
        )?;
    }
    let markdown = normalize_markdown(&document.content)?;
    let visible_characters = markdown
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    if visible_characters == 0
        && spec.processing_class != ProcessingClass::ImageRich
        && spec.format != DocumentFormat::Pdf
    {
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
        format: spec.format,
        detected_content_type: spec.canonical_mime.to_string(),
        content_kind: ParsedContentKind::Markdown,
        markdown,
        markdown_sha256,
        page_count,
        sheet_count,
        sheet_names,
        metadata,
        warnings,
        vision_candidates,
        parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
    })
}

fn plain_text_success(
    spec: &crate::DocumentFormatSpec,
    source: &str,
) -> Result<ParseSuccess, DocumentParserError> {
    let markdown = normalize_markdown(source)?;
    if markdown.chars().all(char::is_whitespace) {
        return Err(error(DocumentErrorCode::DocumentEmpty));
    }
    Ok(ParseSuccess {
        version: PROTOCOL_VERSION,
        format: spec.format,
        detected_content_type: spec.canonical_mime.to_owned(),
        content_kind: ParsedContentKind::PlainText,
        markdown_sha256: sha256_hex(markdown.as_bytes()),
        markdown,
        page_count: None,
        sheet_count: None,
        sheet_names: None,
        metadata: DocumentMetadata::default(),
        warnings: Vec::new(),
        vision_candidates: Vec::new(),
        parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
    })
}

fn decode_plain_text(bytes: &[u8]) -> Result<String, DocumentParserError> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| error(DocumentErrorCode::ContentTypeMismatch))
}

fn required_tessdata_path(request: &ParseRequest) -> Result<PathBuf, DocumentParserError> {
    let path = request
        .header
        .tessdata_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    crate::verify_tessdata_dir(&path)
        .map_err(|_| error(DocumentErrorCode::DocumentOcrUnavailable))?;
    Ok(path)
}

fn extraction_config(
    include_document_furniture: bool,
    tessdata_path: Option<PathBuf>,
) -> ExtractionConfig {
    let ocr_enabled = tessdata_path.is_some();
    let ocr = tessdata_path.map(|tessdata_path| OcrConfig {
        enabled: true,
        backend: "tesseract".to_owned(),
        // Xberg's outer config validator accepts only ISO-shaped codes, while
        // Tesseract's canonical Simplified/Traditional model names contain
        // identifiers rejected by that validator. The materializer provides
        // digest-checked zho/chinese_cht hard-link names for the exact bytes.
        language: "eng+zho+chinese_cht"
            .split('+')
            .map(str::to_owned)
            .collect(),
        auto_rotate: false,
        // Open Compute owns the only reusable parsed-document cache. Xberg's
        // process-global OCR cache is neither account-scoped nor bounded and
        // would attempt a forbidden regular-file write under RLIMIT_FSIZE=0.
        backend_options: Some(serde_json::json!({"use_cache": false})),
        tessdata_path: Some(tessdata_path),
        ..OcrConfig::default()
    });
    let mut config = ExtractionConfig {
        use_cache: false,
        enable_quality_processing: false,
        ocr,
        disable_ocr: !ocr_enabled,
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
            max_pages: Some(1_000),
        }),
        content_filter: Some(ContentFilterConfig {
            include_headers: include_document_furniture,
            include_footers: include_document_furniture,
            include_footnotes: include_document_furniture,
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
    config
}

fn image_candidate(
    format: DocumentFormat,
    bytes: &[u8],
    source_page: Option<u32>,
) -> Result<VisionCandidate, DocumentParserError> {
    let image = if format == DocumentFormat::Svg {
        render_svg(bytes)?
    } else {
        decode_raster(bytes)?
    };
    let image = resize_for_vision(image);
    let (jpeg, width, height) = encode_bounded_vision_jpeg(image)?;
    Ok(VisionCandidate {
        data_base64: base64::engine::general_purpose::STANDARD.encode(&jpeg),
        mime_type: "image/jpeg".to_owned(),
        width,
        height,
        sha256: sha256_hex(&jpeg),
        source_page,
        ocr_performed: true,
        ocr_confidence_milli: None,
    })
}

fn pdf_vision_candidates(
    bytes: &[u8],
    limit: usize,
) -> Result<Vec<VisionCandidate>, DocumentParserError> {
    let document = xberg_native_pdf::PdfDocument::from_bytes(bytes.to_vec())
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let page_count = document
        .page_count()
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?
        .min(limit);
    let mut options = xberg_native_pdf::rendering::RenderOptions::default();
    options.format = xberg_native_pdf::rendering::ImageFormat::Jpeg;
    options.jpeg_quality = 90;
    (0..page_count)
        .map(|page| {
            let rendered = xberg_native_pdf::rendering::render_page_fit(
                &document,
                page,
                VISION_MAX_WIDTH,
                VISION_MAX_HEIGHT,
                &options,
            )
            .map_err(|_| error(DocumentErrorCode::DocumentParseFailed))?;
            let source_page = u32::try_from(page + 1)
                .map_err(|_| error(DocumentErrorCode::DocumentLimitExceeded))?;
            image_candidate(DocumentFormat::Jpeg, &rendered.data, Some(source_page))
        })
        .collect()
}

fn decode_raster(bytes: &[u8]) -> Result<DynamicImage, DocumentParserError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let (width, height) = decoder.dimensions();
    validate_image_dimensions(width, height)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    image.apply_orientation(orientation);
    validate_image_dimensions(image.width(), image.height())?;
    Ok(image)
}

fn render_svg(bytes: &[u8]) -> Result<DynamicImage, DocumentParserError> {
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    let size = tree.size().to_int_size();
    validate_image_dimensions(size.width(), size.height())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| error(DocumentErrorCode::DocumentLimitExceeded))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let rgba = image::RgbaImage::from_raw(size.width(), size.height(), pixmap.take())
        .ok_or_else(|| error(DocumentErrorCode::DocumentInvalid))?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), DocumentParserError> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if width == 0 || height == 0 || width > 8_192 || height > 8_192 || pixels > MAX_IMAGE_PIXELS {
        return Err(error(DocumentErrorCode::DocumentLimitExceeded));
    }
    Ok(())
}

fn resize_for_vision(image: DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    let pixel_scale = (VISION_MAX_PIXELS as f64 / f64::from(width) / f64::from(height)).sqrt();
    let scale = 1_f64
        .min(f64::from(VISION_MAX_WIDTH) / f64::from(width))
        .min(f64::from(VISION_MAX_HEIGHT) / f64::from(height))
        .min(pixel_scale);
    if scale >= 1.0 {
        return image;
    }
    let target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    image.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::Lanczos3,
    )
}

fn encode_bounded_vision_jpeg(
    mut image: DynamicImage,
) -> Result<(Vec<u8>, u32, u32), DocumentParserError> {
    loop {
        let jpeg = encode_white_jpeg(&image)?;
        if jpeg.len() <= VISION_MAX_BYTES {
            return Ok((jpeg, image.width(), image.height()));
        }
        if image.width() == 1 && image.height() == 1 {
            return Err(error(DocumentErrorCode::DocumentVisionInputTooLarge));
        }
        let width = (image.width().saturating_mul(3) / 4).max(1);
        let height = (image.height().saturating_mul(3) / 4).max(1);
        image = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    }
}

fn encode_white_jpeg(image: &DynamicImage) -> Result<Vec<u8>, DocumentParserError> {
    let rgba = image.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (source, target) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = u16::from(source[3]);
        for channel in 0..3 {
            let value = (u16::from(source[channel]) * alpha + 255 * (255 - alpha) + 127) / 255;
            target[channel] = u8::try_from(value).unwrap_or(255);
        }
    }
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 90)
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| error(DocumentErrorCode::DocumentInvalid))?;
    Ok(output)
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
/// input and output. OCR requests read only the parent-verified tessdata directory;
/// the child performs no network I/O.
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
            Ok(request) => match Box::pin(parse_document(&request)).await {
                Ok(success) => ParseOutput::Success(Box::new(success)),
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
