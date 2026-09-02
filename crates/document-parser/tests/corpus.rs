//! Independent fixed-corpus acceptance for the frozen document parser contract.

use open_compute_document_parser::{
    InputHeader, PARSER_CONTRACT_SHA256, ParseRequest, decode_input_frame, parse_document,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Manifest {
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
struct Fixture {
    id: String,
    path: String,
    sha256: String,
    size_bytes: u64,
    mime: String,
    expected_status: String,
    oracle: String,
}

#[derive(Deserialize)]
struct Oracle {
    status: String,
    error: Option<String>,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
    retrieval_queries: Vec<RetrievalQuery>,
    min_normalized_chars: usize,
    max_normalized_chars: usize,
    structure: Structure,
}

#[derive(Deserialize)]
struct RetrievalQuery {
    query: String,
    expected_fixture_id: String,
}

#[derive(Deserialize)]
struct Structure {
    min_headings: usize,
    min_tables: usize,
    page_count: Option<u32>,
    sheet_names: Vec<String>,
}

#[derive(Deserialize)]
struct HostileManifest {
    cases: Vec<HostileCase>,
}

#[derive(Deserialize)]
struct HostileCase {
    path: String,
    size_bytes: u64,
    sha256: String,
    expected_error: String,
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_corpus_matches_reviewed_oracles() {
    let root = fixture_root();
    let manifest: Manifest = read_json(&root.join("manifest.json"));
    let mut golden_digests: BTreeMap<String, String> = read_json(&root.join("golden-digests.json"));
    assert!(manifest.fixtures.len() >= 30);
    let mut failures = Vec::new();

    for fixture in manifest.fixtures {
        let bytes = std::fs::read(root.join(&fixture.path)).unwrap();
        if bytes.len() as u64 != fixture.size_bytes {
            failures.push(format!("{} size mismatch", fixture.id));
            continue;
        }
        if sha256_hex(&bytes) != fixture.sha256 {
            failures.push(format!("{} digest mismatch", fixture.id));
            continue;
        }
        let oracle: Oracle = read_json(&root.join(&fixture.oracle));
        if fixture.expected_status != oracle.error.as_deref().unwrap_or("ok") {
            failures.push(format!("{} manifest/oracle status mismatch", fixture.id));
            continue;
        }
        let filename = Path::new(&fixture.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_string();
        let request = ParseRequest {
            header: InputHeader {
                request_id: format!("fixture-{}", fixture.id),
                filename,
                declared_content_type: fixture.mime,
                content_sha256: fixture.sha256,
                parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
                html_options: None,
            },
            body: bytes,
        };

        match parse_document(&request).await {
            Ok(output) => {
                if oracle.status != "ok" {
                    failures.push(format!("{} unexpectedly succeeded", fixture.id));
                    continue;
                }
                let character_count = output.markdown.chars().count();
                if !(oracle.min_normalized_chars..=oracle.max_normalized_chars)
                    .contains(&character_count)
                {
                    failures.push(format!(
                        "{} normalized character count {character_count}",
                        fixture.id
                    ));
                }
                let searchable = semantic_text(&output.markdown);
                for required in oracle.must_contain {
                    if !output.markdown.contains(&required)
                        && !searchable.contains(&semantic_text(&required))
                    {
                        failures.push(format!(
                            "{} missing reviewed fragment {required:?}; output={:?}",
                            fixture.id, output.markdown
                        ));
                    }
                }
                for forbidden in oracle.must_not_contain {
                    if output.markdown.contains(&forbidden)
                        || searchable.contains(&semantic_text(&forbidden))
                    {
                        failures.push(format!(
                            "{} leaked forbidden fragment {forbidden:?}",
                            fixture.id
                        ));
                    }
                }
                for retrieval in oracle.retrieval_queries {
                    if retrieval.expected_fixture_id != fixture.id {
                        failures.push(format!(
                            "{} retrieval oracle points at {}",
                            fixture.id, retrieval.expected_fixture_id
                        ));
                    }
                    let query = semantic_text(&retrieval.query);
                    if query.is_empty() || !searchable.contains(&query) {
                        failures.push(format!(
                            "{} retrieval query {:?} is absent from normalized output",
                            fixture.id, retrieval.query
                        ));
                    }
                }
                if fixture.path.ends_with(".pdf")
                    && let Some(page_count) = oracle.structure.page_count
                    && output.page_count != Some(page_count)
                {
                    failures.push(format!(
                        "{} page count {:?} != {page_count}",
                        fixture.id, output.page_count
                    ));
                }
                if !oracle.structure.sheet_names.is_empty() {
                    let expected = u32::try_from(oracle.structure.sheet_names.len()).ok();
                    if output.sheet_count != expected {
                        failures.push(format!(
                            "{} sheet count {:?} != {expected:?}",
                            fixture.id, output.sheet_count
                        ));
                    }
                    if output.sheet_names.as_deref()
                        != Some(oracle.structure.sheet_names.as_slice())
                    {
                        failures.push(format!(
                            "{} sheet names {:?} != {:?}",
                            fixture.id, output.sheet_names, oracle.structure.sheet_names
                        ));
                    }
                }
                let headings = output
                    .markdown
                    .lines()
                    .filter(|line| line.trim_start().starts_with('#'))
                    .count();
                if headings < oracle.structure.min_headings {
                    failures.push(format!(
                        "{} heading count {headings} < {}",
                        fixture.id, oracle.structure.min_headings
                    ));
                }
                let tables = markdown_table_count(&output.markdown);
                if tables < oracle.structure.min_tables {
                    failures.push(format!(
                        "{} table count {tables} < {}",
                        fixture.id, oracle.structure.min_tables
                    ));
                }
                match golden_digests.remove(&fixture.id) {
                    Some(digest) if output.markdown_sha256 != digest => failures.push(format!(
                        "{} golden digest {} != {}",
                        fixture.id, output.markdown_sha256, digest
                    )),
                    Some(_) => {}
                    None => failures.push(format!("{} lacks a golden digest", fixture.id)),
                }
            }
            Err(parser_error) => {
                let expected = oracle.error.as_deref().unwrap_or("ok");
                if parser_error.code.as_str() != expected {
                    failures.push(format!(
                        "{} expected {expected}, got {parser_error}",
                        fixture.id
                    ));
                }
            }
        }
    }
    if !golden_digests.is_empty() {
        failures.push(format!(
            "golden digests have no successful fixture: {:?}",
            golden_digests.keys().collect::<Vec<_>>()
        ));
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn deterministic_hostile_corpus_is_rejected_without_panics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("test/fuzz/corpus/document-parser");
    let manifest: HostileManifest = read_json(&root.join("manifest.json"));
    assert!(manifest.cases.len() >= 15);
    let mut failures = Vec::new();
    for case in manifest.cases {
        let bytes = std::fs::read(root.join(&case.path)).unwrap();
        assert_eq!(bytes.len() as u64, case.size_bytes, "{} size", case.path);
        assert_eq!(sha256_hex(&bytes), case.sha256, "{} digest", case.path);
        if case.path.ends_with(".bin") {
            let error = decode_input_frame(&bytes).expect_err(&format!(
                "{} unexpectedly passed frame admission",
                case.path
            ));
            if error.code.as_str() != case.expected_error {
                failures.push(format!(
                    "{} frame error {} != {}",
                    case.path,
                    error.code.as_str(),
                    case.expected_error
                ));
            }
            continue;
        }
        let extension = Path::new(&case.path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap();
        let mime = match extension {
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "odt" => "application/vnd.oasis.opendocument.text",
            "pdf" => "application/pdf",
            "xls" => "application/vnd.ms-excel",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            _ => panic!("unrecognized hostile extension: {extension}"),
        };
        let request = ParseRequest {
            header: InputHeader {
                request_id: format!("hostile-{}", case.path),
                filename: case.path.clone(),
                declared_content_type: mime.to_string(),
                content_sha256: case.sha256,
                parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
                html_options: None,
            },
            body: bytes,
        };
        let error = parse_document(&request)
            .await
            .expect_err(&format!("{} unexpectedly parsed", case.path));
        if error.code.as_str() != case.expected_error {
            failures.push(format!(
                "{} parser error {} != {}",
                case.path,
                error.code.as_str(),
                case.expected_error
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("test/fixtures/document-parser")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let bytes = std::fs::read(path).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn semantic_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_table_count(markdown: &str) -> usize {
    markdown
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with('|')
                && line.ends_with('|')
                && line.contains("---")
                && line
                    .bytes()
                    .all(|byte| matches!(byte, b'|' | b'-' | b':' | b' '))
        })
        .count()
}
