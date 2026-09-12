use super::*;
use base64::Engine as _;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use std::io::{Cursor, Write as _};

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(body).unwrap();
    encoder.finish().unwrap()
}

fn request(filename: &str, mime: &str, body: &[u8]) -> ParseRequest {
    ParseRequest {
        header: InputHeader {
            request_id: "request-1".to_string(),
            filename: filename.to_string(),
            declared_content_type: mime.to_string(),
            content_sha256: sha256_hex(body),
            parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
            max_input_bytes: MAX_DOCUMENT_BYTES as u64,
            tessdata_path: None,
            vision_candidate_limit: 0,
            html_options: None,
        },
        body: body.to_vec(),
    }
}

#[test]
fn parser_contract_manifest_is_exact() {
    assert_eq!(
        sha256_hex(PARSER_CONTRACT_MANIFEST.as_bytes()),
        PARSER_CONTRACT_SHA256
    );
    assert_eq!(MAX_DOCUMENT_BYTES, 67_108_864);
    assert_eq!(MAX_HEADER_BYTES, 16_384);
    assert_eq!(MAX_MARKDOWN_BYTES, 16_777_216);
    assert_eq!(MAX_OUTPUT_FRAME_BYTES, 67_108_864);
}

fn success(markdown: &str) -> ParseSuccess {
    ParseSuccess {
        version: PROTOCOL_VERSION,
        format: DocumentFormat::Text,
        detected_content_type: DocumentFormat::Text.mime_type().to_string(),
        content_kind: ParsedContentKind::PlainText,
        markdown: markdown.to_string(),
        markdown_sha256: sha256_hex(markdown.as_bytes()),
        page_count: None,
        sheet_count: None,
        sheet_names: None,
        metadata: DocumentMetadata::default(),
        warnings: Vec::new(),
        vision_candidates: Vec::new(),
        parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
    }
}

#[test]
fn input_frame_round_trips_canonical_request() {
    let expected = request("notes.txt", "text/plain", b"hello");
    let encoded = encode_input_frame(&expected).unwrap();
    assert_eq!(&encoded[..4], b"OCDP");
    assert_eq!(decode_input_frame(&encoded).unwrap(), expected);
}

#[test]
fn input_frame_rejects_hostile_preludes_and_lengths() {
    let valid = encode_input_frame(&request("notes.txt", "text/plain", b"hello")).unwrap();
    for mutation in [0_usize, 4, 5] {
        let mut frame = valid.clone();
        frame[mutation] ^= 0xff;
        assert_eq!(
            decode_input_frame(&frame).unwrap_err().code,
            DocumentErrorCode::InvalidFrame
        );
    }

    let mut overflow = valid.clone();
    overflow[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        decode_input_frame(&overflow).unwrap_err().code,
        DocumentErrorCode::DocumentLimitExceeded
    );
    let mut short = valid.clone();
    let _ = short.pop();
    assert_eq!(
        decode_input_frame(&short).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );
    let mut trailing = valid;
    trailing.push(0);
    assert_eq!(
        decode_input_frame(&trailing).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );
}

#[test]
fn input_frame_rejects_noncanonical_and_duplicate_json() {
    let expected = request("notes.txt", "text/plain", b"hello");
    let canonical = serde_json::to_vec(&expected.header).unwrap();
    let mut spaced = Vec::new();
    spaced.extend_from_slice(b"OCDP\0\x01");
    spaced.extend_from_slice(&u32::try_from(canonical.len() + 1).unwrap().to_be_bytes());
    spaced.extend_from_slice(&5_u32.to_be_bytes());
    spaced.push(b' ');
    spaced.extend_from_slice(&canonical);
    spaced.extend_from_slice(b"hello");
    assert_eq!(
        decode_input_frame(&spaced).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );

    let duplicate = canonical
        .strip_suffix(b"}")
        .unwrap()
        .iter()
        .copied()
        .chain(b",\"request_id\":\"other\"}".iter().copied())
        .collect::<Vec<_>>();
    let mut duplicate_frame = Vec::new();
    duplicate_frame.extend_from_slice(b"OCDP\0\x01");
    duplicate_frame.extend_from_slice(&u32::try_from(duplicate.len()).unwrap().to_be_bytes());
    duplicate_frame.extend_from_slice(&5_u32.to_be_bytes());
    duplicate_frame.extend_from_slice(&duplicate);
    duplicate_frame.extend_from_slice(b"hello");
    assert_eq!(
        decode_input_frame(&duplicate_frame).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );
}

#[test]
fn input_frame_rejects_digest_contract_and_unsafe_name() {
    let mut digest = request("notes.txt", "text/plain", b"hello");
    digest.header.content_sha256 = "0".repeat(64);
    assert_eq!(
        encode_input_frame(&digest).unwrap_err().code,
        DocumentErrorCode::ContentDigestMismatch
    );

    let mut contract = request("notes.txt", "text/plain", b"hello");
    contract.header.parser_contract_sha256 = "0".repeat(64);
    assert_eq!(
        encode_input_frame(&contract).unwrap_err().code,
        DocumentErrorCode::ParserContractMismatch
    );

    let unsafe_name = request("../notes.txt", "text/plain", b"hello");
    assert_eq!(
        encode_input_frame(&unsafe_name).unwrap_err().code,
        DocumentErrorCode::InvalidRequest
    );

    let mut non_html_options = request("notes.txt", "text/plain", b"hello");
    non_html_options.header.html_options = Some(HtmlConversionOptions {
        hostname: Some("example.com".to_owned()),
        css_selector: None,
    });
    assert_eq!(
        admit_document(&non_html_options.header, &non_html_options.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::InvalidRequest
    );
}

#[test]
fn output_frame_round_trips_and_revalidates_digest() {
    let candidate_bytes = b"bounded-jpeg";
    let mut successful = success("hello\n");
    successful.vision_candidates.push(VisionCandidate {
        data_base64: base64::engine::general_purpose::STANDARD.encode(candidate_bytes),
        mime_type: "image/jpeg".to_owned(),
        width: 1,
        height: 1,
        sha256: sha256_hex(candidate_bytes),
        source_page: Some(1),
        ocr_performed: true,
        ocr_confidence_milli: Some(50_000),
    });
    let output = ParseOutput::Success(Box::new(successful));
    let encoded = encode_output_frame(&output).unwrap();
    assert_eq!(decode_output_frame(&encoded).unwrap(), output);

    let mut invalid = success("hello\n");
    invalid.markdown_sha256 = "0".repeat(64);
    assert_eq!(
        encode_output_frame(&ParseOutput::Success(Box::new(invalid)))
            .unwrap_err()
            .code,
        DocumentErrorCode::InvalidFrame
    );
}

#[test]
fn output_frame_rejects_unknown_fields_and_trailing_body() {
    let mut encoded =
        encode_output_frame(&ParseOutput::Success(Box::new(success("hello\n")))).unwrap();
    let json_length = u32::from_be_bytes(encoded[6..10].try_into().unwrap()) as usize;
    let mut json: serde_json::Value = serde_json::from_slice(&encoded[14..]).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("privatePath".to_string(), serde_json::json!("/secret"));
    let json = serde_json::to_vec(&json).unwrap();
    encoded.clear();
    encoded.extend_from_slice(b"OCDP\0\x01");
    encoded.extend_from_slice(&u32::try_from(json.len()).unwrap().to_be_bytes());
    encoded.extend_from_slice(&0_u32.to_be_bytes());
    encoded.extend_from_slice(&json);
    assert_ne!(json_length, 0);
    assert_eq!(
        decode_output_frame(&encoded).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );

    let mut body =
        encode_output_frame(&ParseOutput::Success(Box::new(success("hello\n")))).unwrap();
    body[10..14].copy_from_slice(&1_u32.to_be_bytes());
    body.push(0);
    assert_eq!(
        decode_output_frame(&body).unwrap_err().code,
        DocumentErrorCode::InvalidFrame
    );
}

#[test]
fn output_frame_rejects_contract_warning_sheet_metadata_and_error_drift() {
    let mut cases = Vec::new();
    let mut contract = success("hello\n");
    contract.version = 2;
    cases.push(contract);
    let mut mime = success("hello\n");
    mime.detected_content_type = "application/octet-stream".to_string();
    cases.push(mime);
    let mut controls = success("hello\n");
    controls.markdown = "bad\0text".to_string();
    controls.markdown_sha256 = sha256_hex(controls.markdown.as_bytes());
    cases.push(controls);
    let mut warning = success("hello\n");
    warning.warnings = vec!["lowercase".to_string()];
    cases.push(warning);
    let mut sheets = success("hello\n");
    sheets.sheet_count = Some(2);
    sheets.sheet_names = Some(vec!["only-one".to_string()]);
    cases.push(sheets);
    let mut metadata = success("hello\n");
    metadata.metadata.title = Some("bad\0title".to_string());
    cases.push(metadata);
    for (index, case) in cases.into_iter().enumerate() {
        assert_eq!(
            encode_output_frame(&ParseOutput::Success(Box::new(case)))
                .unwrap_err()
                .code,
            if index < 2 {
                DocumentErrorCode::ParserContractMismatch
            } else if index == 5 {
                DocumentErrorCode::DocumentLimitExceeded
            } else {
                DocumentErrorCode::InvalidFrame
            }
        );
    }

    let mut failure = ParseFailure::from(error(DocumentErrorCode::DocumentInvalid));
    failure.error.message = "upstream leaked path".to_string();
    assert_eq!(
        encode_output_frame(&ParseOutput::Error(failure))
            .unwrap_err()
            .code,
        DocumentErrorCode::InvalidFrame
    );
    let mut failure = ParseFailure::from(error(DocumentErrorCode::DocumentInvalid));
    failure.version = 2;
    assert_eq!(
        encode_output_frame(&ParseOutput::Error(failure))
            .unwrap_err()
            .code,
        DocumentErrorCode::ParserContractMismatch
    );
}

#[test]
fn admission_is_closed_and_cross_checks_content() {
    let text = request("NOTES.TXT", "text/plain", b"hello");
    assert_eq!(
        admit_document(&text.header, &text.body).unwrap().format,
        DocumentFormat::Text
    );

    let renamed = request("notes.pdf", "application/pdf", b"hello");
    assert_eq!(
        admit_document(&renamed.header, &renamed.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::ContentTypeMismatch
    );
    let legacy_word = request("notes.doc", "application/msword", b"hello");
    assert_eq!(
        admit_document(&legacy_word.header, &legacy_word.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::UnsupportedContentType
    );
    let broken_zip = request(
        "notes.docx",
        DocumentFormat::Docx.mime_type(),
        b"PK\x03\x04not-a-zip",
    );
    assert_eq!(
        admit_document(&broken_zip.header, &broken_zip.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::DocumentInvalid
    );
    let broken_ole = request(
        "sheet.xls",
        DocumentFormat::Xls.mime_type(),
        b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1bad",
    );
    assert_eq!(
        admit_document(&broken_ole.header, &broken_ole.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::DocumentInvalid
    );
}

#[test]
fn admission_rejects_size_names_mime_and_utf8_shape_before_parsing() {
    let empty = request("empty.txt", "text/plain", b"");
    assert_eq!(
        admit_document(&empty.header, &empty.body).unwrap_err().code,
        DocumentErrorCode::DocumentEmpty
    );
    let large_body = vec![b'x'; MAX_DOCUMENT_BYTES + 1];
    let large = request("large.txt", "text/plain", &large_body);
    assert_eq!(
        admit_document(&large.header, &large.body).unwrap_err().code,
        DocumentErrorCode::DocumentLimitExceeded
    );
    let extensionless = request("README", "text/plain", b"text");
    assert_eq!(
        admit_document(&extensionless.header, &extensionless.body)
            .unwrap()
            .format,
        DocumentFormat::Text
    );
    for filename in [".txt", "name.", "..", "bad/name.txt", "bad\\name.txt"] {
        let value = request(filename, "text/plain", b"text");
        assert!(
            admit_document(&value.header, &value.body).is_err(),
            "{filename}"
        );
    }
    let wrong_mime = request("note.txt", "text/markdown", b"text");
    assert_eq!(
        admit_document(&wrong_mime.header, &wrong_mime.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::ContentTypeMismatch
    );
    let generic_mime = request("note.txt", "application/octet-stream", b"text");
    assert_eq!(
        admit_document(&generic_mime.header, &generic_mime.body)
            .unwrap()
            .format,
        DocumentFormat::Text
    );
    let extensionless_generic = request("README", "application/octet-stream", b"text");
    assert_eq!(
        admit_document(&extensionless_generic.header, &extensionless_generic.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::UnsupportedContentType
    );
    for (filename, mime, body, code) in [
        (
            "bad.txt",
            "text/plain",
            &b"bad\0text"[..],
            DocumentErrorCode::DocumentInvalid,
        ),
        (
            "bad.txt",
            "text/plain",
            &b"\xff"[..],
            DocumentErrorCode::ContentTypeMismatch,
        ),
        (
            "bad.html",
            "text/html",
            &b"not html"[..],
            DocumentErrorCode::ContentTypeMismatch,
        ),
        (
            "bad.xml",
            "application/xml",
            &b"not xml"[..],
            DocumentErrorCode::ContentTypeMismatch,
        ),
        (
            "bad.pdf",
            "application/pdf",
            &b"not pdf"[..],
            DocumentErrorCode::ContentTypeMismatch,
        ),
        (
            "bad.xls",
            "application/vnd.ms-excel",
            &b"not ole"[..],
            DocumentErrorCode::ContentTypeMismatch,
        ),
    ] {
        let value = request(filename, mime, body);
        assert_eq!(
            admit_document(&value.header, &value.body).unwrap_err().code,
            code,
            "{filename}"
        );
    }
}

#[test]
fn zip_container_identity_is_format_specific() {
    for (filename, mime, entries) in [
        (
            "doc.docx",
            DocumentFormat::Docx.mime_type(),
            vec![
                ("[Content_Types].xml", b"types".as_slice()),
                ("word/document.xml", b"doc"),
            ],
        ),
        (
            "sheet.xlsx",
            DocumentFormat::Xlsx.mime_type(),
            vec![
                ("[Content_Types].xml", b"types".as_slice()),
                ("xl/workbook.xml", b"book"),
            ],
        ),
        (
            "macro.xlsm",
            DocumentFormat::Xlsm.mime_type(),
            vec![
                ("[Content_Types].xml", b"types".as_slice()),
                ("xl/workbook.xml", b"book"),
            ],
        ),
        (
            "doc.odt",
            DocumentFormat::Odt.mime_type(),
            vec![
                (
                    "mimetype",
                    b"application/vnd.oasis.opendocument.text".as_slice(),
                ),
                ("content.xml", b"doc"),
            ],
        ),
        (
            "sheet.ods",
            DocumentFormat::Ods.mime_type(),
            vec![
                (
                    "mimetype",
                    b"application/vnd.oasis.opendocument.spreadsheet".as_slice(),
                ),
                ("content.xml", b"sheet"),
            ],
        ),
    ] {
        let body = zip(&entries);
        let value = request(filename, mime, &body);
        assert_eq!(
            admit_document(&value.header, &value.body)
                .unwrap()
                .canonical_mime,
            mime
        );
    }
    let body = zip(&[
        ("[Content_Types].xml", b"types"),
        ("xl/workbook.xml", b"book"),
    ]);
    let renamed = request("doc.docx", DocumentFormat::Docx.mime_type(), &body);
    assert_eq!(
        admit_document(&renamed.header, &renamed.body)
            .unwrap_err()
            .code,
        DocumentErrorCode::ContentTypeMismatch
    );
    let oversized_mimetype = vec![b'x'; 129];
    let body = zip(&[
        ("mimetype", oversized_mimetype.as_slice()),
        ("content.xml", b"doc"),
    ]);
    let odt = request("doc.odt", DocumentFormat::Odt.mime_type(), &body);
    assert_eq!(
        admit_document(&odt.header, &odt.body).unwrap_err().code,
        DocumentErrorCode::DocumentLimitExceeded
    );
}

#[test]
fn exact_dotfiles_compound_gzip_and_unicode_names_are_admitted() {
    for filename in [".env", ".GITIGNORE", ".editorconfig"] {
        let value = request(filename, "text/plain", b"key=value\n");
        assert_eq!(
            admit_document(&value.header, &value.body).unwrap().format,
            DocumentFormat::Text
        );
    }
    let encoded = gzip(b"first\n");
    let value = request("服务.LOG.GZ", "application/gzip; charset=binary", &encoded);
    assert_eq!(
        admit_document(&value.header, &value.body).unwrap().format,
        DocumentFormat::GzipText
    );
    assert_eq!(decode_gzip_text(&encoded).unwrap(), b"first\n");
}

#[test]
fn gzip_members_trailing_data_and_ratio_are_bounded() {
    let mut members = gzip(b"one\n");
    members.extend(gzip(b"two\n"));
    assert_eq!(decode_gzip_text(&members).unwrap(), b"one\ntwo\n");

    let mut trailing = gzip(b"text\n");
    trailing.extend(b"not-gzip");
    assert_eq!(
        decode_gzip_text(&trailing).unwrap_err().code,
        DocumentErrorCode::DocumentInvalid
    );

    let bomb = gzip(&vec![b'x'; 64 * 1024]);
    assert_eq!(
        decode_gzip_text(&bomb).unwrap_err().code,
        DocumentErrorCode::DocumentLimitExceeded
    );
}

#[test]
fn svg_allows_namespace_and_internal_references_but_rejects_external_content() {
    let internal = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs><path id="p" d="M0 0h10v10z"/></defs><use href="#p"/><rect fill="url(#p)"/></svg>"##;
    let value = request("diagram.svg", "image/svg+xml", internal);
    assert_eq!(
        admit_document(&value.header, &value.body).unwrap().format,
        DocumentFormat::Svg
    );
    for external in [
        br#"<svg><image href="https://example.com/a.png"/></svg>"#.as_slice(),
        br#"<svg><use href="local.svg#shape"/></svg>"#.as_slice(),
        br#"<svg><use href=#shape/></svg>"#.as_slice(),
        br##"<svg><use href="#shape/></svg>"##.as_slice(),
        br#"<svg><rect fill="url(https://example.com/a.svg)"/></svg>"#.as_slice(),
        br#"<svg><script>noop()</script></svg>"#.as_slice(),
    ] {
        let value = request("diagram.svg", "image/svg+xml", external);
        assert_eq!(
            admit_document(&value.header, &value.body).unwrap_err().code,
            DocumentErrorCode::DocumentInvalid
        );
    }
}

#[test]
fn format_registry_is_complete_unique_and_deterministic() {
    let formats = markdown_conversion_formats();
    assert_eq!(DOCUMENT_FORMATS.len(), 62);
    assert_eq!(ai_search_formats().len(), 59);
    assert_eq!(formats.len(), 18);
    let identities = DOCUMENT_FORMATS
        .iter()
        .map(|format| (format.extension, format.canonical_mime))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identities.len(), DOCUMENT_FORMATS.len());
    assert_eq!(
        DOCUMENT_FORMATS
            .iter()
            .filter(|format| !format.ai_search && !format.markdown_conversion)
            .map(|format| format.extension)
            .collect::<Vec<_>>(),
        ["xlsb", "et", "numbers"]
    );
    assert!(
        formats
            .iter()
            .all(|format| !format.extension.starts_with('.') && !format.canonical_mime.is_empty())
    );
    assert!(DOCUMENT_FORMATS.iter().all(|spec| {
        !std::hint::black_box(spec.format).extension().is_empty()
            && !std::hint::black_box(spec.format).mime_type().is_empty()
    }));
}

#[test]
fn raster_admission_covers_every_supported_magic_and_dimension_limit() {
    for (extension, mime, format) in [
        ("jpg", "image/jpeg", ImageFormat::Jpeg),
        ("webp", "image/webp", ImageFormat::WebP),
        ("gif", "image/gif", ImageFormat::Gif),
        ("bmp", "image/bmp", ImageFormat::Bmp),
    ] {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([32, 64, 96])))
            .write_to(&mut encoded, format)
            .unwrap();
        let value = request(&format!("image.{extension}"), mime, &encoded.into_inner());
        admit_document(&value.header, &value.body).unwrap();
    }

    let mut oversized = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(ImageBuffer::from_pixel(8_193, 1, Rgb([0, 0, 0])))
        .write_to(&mut oversized, ImageFormat::Bmp)
        .unwrap();
    let value = request("wide.bmp", "image/bmp", &oversized.into_inner());
    assert_eq!(
        admit_document(&value.header, &value.body).unwrap_err().code,
        DocumentErrorCode::DocumentLimitExceeded
    );
}

#[test]
fn tessdata_verification_rejects_non_authoritative_layouts_and_bytes() {
    assert_eq!(
        materialize_tessdata(std::path::Path::new("relative"))
            .unwrap_err()
            .code,
        DocumentErrorCode::DocumentOcrUnavailable
    );
    let temporary = tempfile::tempdir().unwrap();
    let path = materialize_tessdata(temporary.path()).unwrap();
    std::fs::write(path.join("unexpected.traineddata"), b"extra").unwrap();
    assert_eq!(
        verify_tessdata_dir(&path).unwrap_err().code,
        DocumentErrorCode::DocumentOcrUnavailable
    );
    std::fs::remove_file(path.join("unexpected.traineddata")).unwrap();
    std::fs::write(path.join("eng.traineddata"), b"corrupt").unwrap();
    assert_eq!(
        verify_tessdata_dir(&path).unwrap_err().code,
        DocumentErrorCode::DocumentOcrUnavailable
    );
}

#[tokio::test]
async fn plain_text_parse_is_normalized_and_deterministic() {
    let request = request(
        "notes.txt",
        "text/plain",
        "Cafe\u{301}\r\n\r\n\r\ntext\tline".as_bytes(),
    );
    let first = parse_document(&request).await.unwrap();
    let second = parse_document(&request).await.unwrap();
    assert_eq!(first, second);
    assert!(first.markdown.contains("Café"));
    assert!(!first.markdown.contains('\r'));
    assert!(!first.markdown.contains('\t'));
    assert_eq!(first.markdown_sha256, sha256_hex(first.markdown.as_bytes()));
}

#[tokio::test]
async fn maintained_text_formats_have_two_offline_success_cases_each() {
    for (filename, mime, body, expected) in [
        ("one.md", "text/markdown", "# Heading", "Heading"),
        ("two.md", "text/markdown", "Unicode café", "café"),
        ("one.html", "text/html", "<h1>Heading</h1>", "Heading"),
        ("two.html", "text/html", "<p>Unicode café</p>", "café"),
        (
            "one.xml",
            "application/xml",
            "<root>Heading</root>",
            "Heading",
        ),
        (
            "two.xml",
            "application/xml",
            "<root>Unicode café</root>",
            "café",
        ),
        (
            "one.json",
            "application/json",
            "{\"title\":\"Heading\"}",
            "Heading",
        ),
        ("two.json", "application/json", "[\"Unicode café\"]", "café"),
        ("one.csv", "text/csv", "title,value\nHeading,1", "Heading"),
        ("two.csv", "text/csv", "title,value\nUnicode café,2", "café"),
        ("one.txt", "text/plain", "Heading", "Heading"),
        ("two.txt", "text/plain", "Unicode café", "café"),
    ] {
        let output = parse_document(&request(filename, mime, body.as_bytes()))
            .await
            .unwrap_or_else(|parser_error| panic!("{filename}: {parser_error}"));
        assert!(
            output.markdown.contains(expected),
            "{filename}: {:?}",
            output.markdown
        );
    }
}

#[test]
fn html_options_validate_bounded_http_base_and_selector() {
    for hostname in ["example.com/base/", "https://example.com/base/"] {
        HtmlConversionOptions {
            hostname: Some(hostname.to_owned()),
            css_selector: Some("main, article.content".to_owned()),
        }
        .validate()
        .unwrap();
    }
    for hostname in ["file:///etc/passwd", "https://user@example.com/"] {
        assert!(
            HtmlConversionOptions {
                hostname: Some(hostname.to_owned()),
                css_selector: None,
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        HtmlConversionOptions {
            hostname: None,
            css_selector: Some("@import url(x)".to_owned()),
        }
        .validate()
        .is_err()
    );
    for option in [
        HtmlConversionOptions {
            hostname: Some(String::new()),
            css_selector: None,
        },
        HtmlConversionOptions {
            hostname: Some("x".repeat(2049)),
            css_selector: None,
        },
        HtmlConversionOptions {
            hostname: None,
            css_selector: Some(String::new()),
        },
        HtmlConversionOptions {
            hostname: None,
            css_selector: Some("main,".to_string()),
        },
        HtmlConversionOptions {
            hostname: None,
            css_selector: Some("x".repeat(513)),
        },
        HtmlConversionOptions {
            hostname: None,
            css_selector: Some(
                (0..17)
                    .map(|index| format!(".x{index}"))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        },
    ] {
        assert!(option.validate().is_err());
    }
}

#[tokio::test]
async fn html_selector_and_base_resolution_are_applied_without_network() {
    let body = br#"<html><head><base href="https://ignored.example/root/"></head><body><main><a href="../guide">Selected</a></main><aside>Removed</aside></body></html>"#;
    let mut request = request("page.html", "text/html", body);
    request.header.html_options = Some(HtmlConversionOptions {
        hostname: Some("example.com/base/".to_owned()),
        css_selector: Some("main".to_owned()),
    });
    let output = parse_document(&request).await.unwrap();
    assert!(output.markdown.contains("Selected"));
    assert!(output.markdown.contains("https://example.com/guide"));
    assert!(!output.markdown.contains("Removed"));
}

#[tokio::test]
async fn html_document_base_nested_selection_and_empty_selection_are_deterministic() {
    let body = br#"<html><head><base href="https://example.com/root/"></head><body><main><article><a href="guide">Selected</a></article></main></body></html>"#;
    let mut selected = request("page.html", "text/html", body);
    selected.header.html_options = Some(HtmlConversionOptions {
        hostname: None,
        css_selector: Some("main, article".to_string()),
    });
    let output = parse_document(&selected).await.unwrap();
    assert!(output.markdown.contains("https://example.com/root/guide"));
    assert_eq!(output.markdown.matches("Selected").count(), 1);

    let mut empty = request("page.html", "text/html", body);
    empty.header.html_options = Some(HtmlConversionOptions {
        hostname: None,
        css_selector: Some("footer".to_string()),
    });
    assert_eq!(
        parse_document(&empty).await.unwrap_err().code,
        DocumentErrorCode::DocumentEmpty
    );
}

#[test]
fn child_emits_structured_error_for_hostile_frame() {
    let mut output = Vec::new();
    run_child(Cursor::new(b"hostile"), &mut output).unwrap();
    let decoded = decode_output_frame(&output).unwrap();
    let ParseOutput::Error(failure) = decoded else {
        panic!("hostile input unexpectedly parsed");
    };
    assert_eq!(failure.error.code, DocumentErrorCode::InvalidFrame);
    assert!(!failure.error.message.contains("hostile"));
}

#[test]
fn child_parses_one_complete_frame() {
    let frame = encode_input_frame(&request("notes.txt", "text/plain", b"hello world")).unwrap();
    let mut output = Vec::new();
    run_child(Cursor::new(frame), &mut output).unwrap();
    let ParseOutput::Success(success) = decode_output_frame(&output).unwrap() else {
        panic!("valid text frame failed");
    };
    assert!(success.markdown.contains("hello world"));
}

#[test]
fn child_bounds_input_and_error_codes_are_stable_and_content_free() {
    let mut output = Vec::new();
    run_child(
        Cursor::new(vec![0_u8; MAX_DOCUMENT_BYTES + MAX_HEADER_BYTES + 16]),
        &mut output,
    )
    .unwrap();
    let ParseOutput::Error(failure) = decode_output_frame(&output).unwrap() else {
        panic!("oversized child input unexpectedly parsed");
    };
    assert_eq!(failure.error.code, DocumentErrorCode::DocumentLimitExceeded);

    for code in [
        DocumentErrorCode::InvalidFrame,
        DocumentErrorCode::InvalidRequest,
        DocumentErrorCode::DocumentLimitExceeded,
        DocumentErrorCode::ContentDigestMismatch,
        DocumentErrorCode::ParserContractMismatch,
        DocumentErrorCode::UnsupportedContentType,
        DocumentErrorCode::ContentTypeMismatch,
        DocumentErrorCode::DocumentInvalid,
        DocumentErrorCode::DocumentEncrypted,
        DocumentErrorCode::DocumentEmpty,
        DocumentErrorCode::DocumentOcrUnavailable,
        DocumentErrorCode::DocumentVisionInputTooLarge,
        DocumentErrorCode::DocumentParseFailed,
    ] {
        let parser_error = error(code);
        assert!(parser_error.to_string().starts_with(code.as_str()));
        assert!(!parser_error.message.is_empty());
    }
}
