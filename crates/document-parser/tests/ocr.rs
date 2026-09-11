//! Offline OCR and normalized vision-candidate acceptance.

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use open_compute_document_parser::{
    DocumentFormat, InputHeader, MAX_DOCUMENT_BYTES, PARSER_CONTRACT_SHA256, ParseRequest,
    ParsedContentKind, materialize_tessdata, parse_document, verify_tessdata_dir,
};
use sha2::{Digest as _, Sha256};
use std::io::Cursor;

#[tokio::test(flavor = "current_thread")]
async fn png_uses_fixed_offline_ocr_and_emits_bounded_jpeg_candidate() {
    let mut raster = ImageBuffer::from_pixel(320, 120, Rgba([255, 255, 255, 255]));
    for y in 25..95 {
        for x in 30..290 {
            if !(35..=84).contains(&y) || !(40..=279).contains(&x) {
                raster.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
    }
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(raster)
        .write_to(&mut png, ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let temporary = tempfile::tempdir().unwrap();
    let tessdata = materialize_tessdata(temporary.path()).unwrap();
    verify_tessdata_dir(&tessdata).unwrap();
    let request = ParseRequest {
        header: InputHeader {
            request_id: "offline-ocr".to_owned(),
            filename: "scan.png".to_owned(),
            declared_content_type: "image/png".to_owned(),
            content_sha256: hex::encode(Sha256::digest(&png)),
            parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
            max_input_bytes: MAX_DOCUMENT_BYTES as u64,
            tessdata_path: Some(tessdata.to_string_lossy().into_owned()),
            vision_candidate_limit: 1,
            html_options: None,
        },
        body: png,
    };
    let parsed = parse_document(&request).await.unwrap();
    assert_eq!(parsed.format, DocumentFormat::Png);
    assert_eq!(parsed.content_kind, ParsedContentKind::Markdown);
    let candidate = parsed
        .vision_candidates
        .into_iter()
        .next()
        .expect("normalized image candidate");
    assert_eq!(candidate.mime_type, "image/jpeg");
    assert!(candidate.ocr_performed);
    assert!(candidate.width <= 1_280);
    assert!(candidate.height <= 720);
    assert!(candidate.data_base64.len() < 6 * 1024 * 1024);
}

#[tokio::test(flavor = "current_thread")]
async fn scanned_pdf_emits_only_the_bounded_page_candidate() {
    let pdf =
        include_bytes!("../../../test/fixtures/document-parser/corpus/apache-tika/pdf/testOCR.pdf")
            .to_vec();
    let temporary = tempfile::tempdir().unwrap();
    let tessdata = materialize_tessdata(temporary.path()).unwrap();
    let parsed = parse_document(&ParseRequest {
        header: InputHeader {
            request_id: "scanned-pdf-vision".to_owned(),
            filename: "scan.pdf".to_owned(),
            declared_content_type: "application/pdf".to_owned(),
            content_sha256: hex::encode(Sha256::digest(&pdf)),
            parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
            max_input_bytes: MAX_DOCUMENT_BYTES as u64,
            tessdata_path: Some(tessdata.to_string_lossy().into_owned()),
            vision_candidate_limit: 1,
            html_options: None,
        },
        body: pdf,
    })
    .await
    .unwrap();
    assert_eq!(parsed.format, DocumentFormat::Pdf);
    assert!(!parsed.markdown.trim().is_empty());
    assert_eq!(parsed.vision_candidates.len(), 1);
    let candidate = &parsed.vision_candidates[0];
    assert_eq!(candidate.source_page, Some(1));
    assert!(candidate.width <= 1_280);
    assert!(candidate.height <= 720);
}

#[tokio::test(flavor = "current_thread")]
async fn svg_is_rendered_offline_before_ocr_and_candidate_encoding() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="white"/><text x="80" y="180" font-size="96">offline diagram</text></svg>"#;
    let temporary = tempfile::tempdir().unwrap();
    let tessdata = materialize_tessdata(temporary.path()).unwrap();
    let parsed = parse_document(&ParseRequest {
        header: InputHeader {
            request_id: "svg-vision".to_owned(),
            filename: "diagram.svg".to_owned(),
            declared_content_type: "image/svg+xml".to_owned(),
            content_sha256: hex::encode(Sha256::digest(svg)),
            parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
            max_input_bytes: MAX_DOCUMENT_BYTES as u64,
            tessdata_path: Some(tessdata.to_string_lossy().into_owned()),
            vision_candidate_limit: 1,
            html_options: None,
        },
        body: svg.to_vec(),
    })
    .await
    .unwrap();
    assert_eq!(parsed.format, DocumentFormat::Svg);
    let candidate = parsed.vision_candidates.first().unwrap();
    assert!(candidate.width <= 1_280);
    assert!(candidate.height <= 720);
    assert!(candidate.ocr_performed);
}
