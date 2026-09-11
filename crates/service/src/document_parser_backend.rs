//! Version-authorized Markdown Conversion backed by isolated parser children.

mod process;
mod protocol;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, Method};
#[cfg(test)]
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use open_compute_core::{
    AccountId, AiConfig, DocumentParserConfig, ErrorCode, PlatformError, ResolvedVlmModelContract,
    VersionId, WorkerId,
};
use open_compute_document_parser::{
    HtmlConversionOptions, InputHeader, PARSER_CONTRACT_SHA256, ParseOutput, ParseRequest,
    ParseSuccess, ParsedContentKind, VisionCandidate, decode_output_frame, encode_input_frame,
    markdown_conversion_formats, materialize_tessdata,
};
use open_compute_storage::{
    BuiltinBindingKind, PlatformStorage, VersionState, WorkerRepository, version_runtime_features,
};
use process::run_parser_child;
use protocol::*;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use uuid::Uuid;

const ACCOUNT_HEADER: &str = "x-open-compute-account-id";
const WORKER_HEADER: &str = "x-open-compute-worker-id";
const VERSION_HEADER: &str = "x-open-compute-version-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const MAX_NAME_BYTES: usize = 255;
const MAX_MIME_BYTES: usize = 128;

use crate::ai_provider::OpenAiVisionClient;

/// One version-scoped Markdown Conversion service.
pub struct DocumentParserBindingService {
    storage: Arc<PlatformStorage>,
    config: DocumentParserConfig,
    executable: PathBuf,
    tessdata_path: PathBuf,
    vlm_contract: Option<ResolvedVlmModelContract>,
    vlm: Option<OpenAiVisionClient>,
    vlm_semaphore: Arc<Semaphore>,
    global: Arc<Semaphore>,
    accounts: Mutex<HashMap<AccountId, Weak<Semaphore>>>,
    versions: Mutex<HashMap<VersionId, Weak<Semaphore>>>,
}

impl std::fmt::Debug for DocumentParserBindingService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentParserBindingService")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DocumentParserBindingService {
    /// Resolved per-document input limit shared by Markdown Conversion and AI Search.
    #[must_use]
    pub const fn max_input_bytes(&self) -> u64 {
        self.config.max_input_bytes
    }

    /// Digest the complete image-description semantics used by an indexing parse.
    #[must_use]
    pub fn semantic_contract_sha256(
        &self,
        language: &str,
        content_kind: ParsedContentKind,
    ) -> String {
        let mut digest = Sha256::new();
        digest.update(b"open-compute/document-semantic/v1\0");
        digest.update(PARSER_CONTRACT_SHA256.as_bytes());
        digest.update([0]);
        digest.update(match content_kind {
            ParsedContentKind::PlainText => b"plain-text".as_slice(),
            ParsedContentKind::Markdown => b"markdown".as_slice(),
        });
        digest.update([0]);
        digest.update(language.as_bytes());
        digest.update([0]);
        digest.update(
            self.vlm_contract
                .as_ref()
                .map_or("vlm-disabled", |contract| contract.contract_sha256.as_str())
                .as_bytes(),
        );
        hex::encode(digest.finalize())
    }

    /// Digest every fixed parser, conversion, OCR, and VLM input used by AI Search.
    #[must_use]
    pub fn ai_search_cache_contract_sha256(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"open-compute/ai-search-parse-cache-contract/v1\0");
        digest.update(PARSER_CONTRACT_SHA256.as_bytes());
        digest.update(b"\0language=en\0html-options=none\0");
        digest.update(
            self.vlm_contract
                .as_ref()
                .map_or("vlm-disabled", |contract| contract.contract_sha256.as_str())
                .as_bytes(),
        );
        digest.finalize().into()
    }

    /// Compose the binding service with the running `ocd` executable.
    pub fn new(
        storage: Arc<PlatformStorage>,
        config: DocumentParserConfig,
        ai: &AiConfig,
    ) -> Result<Self, PlatformError> {
        let executable = std::env::current_exe().map_err(|_| unavailable())?;
        if !executable.is_absolute() {
            return Err(unavailable());
        }
        Self::with_executable(storage, config, ai, executable)
    }

    /// Compose a service with an explicit executable, primarily for real-process fixtures.
    pub fn with_executable(
        storage: Arc<PlatformStorage>,
        config: DocumentParserConfig,
        ai: &AiConfig,
        executable: PathBuf,
    ) -> Result<Self, PlatformError> {
        let tessdata_path =
            materialize_tessdata(storage.data_dir().root()).map_err(|_| unavailable())?;
        let vlm_contract = ai.resolve_default_vlm_model()?;
        let vlm = vlm_contract
            .as_ref()
            .map(|contract| OpenAiVisionClient::new(ai, contract))
            .transpose()
            .map_err(|_| unavailable())?;
        let max_vlm_in_flight = usize::from(ai.max_vlm_in_flight);
        Ok(Self {
            storage,
            global: Arc::new(Semaphore::new(config.max_concurrency as usize)),
            accounts: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            executable,
            tessdata_path,
            vlm_contract,
            vlm,
            vlm_semaphore: Arc::new(Semaphore::new(max_vlm_in_flight)),
            config,
        })
    }

    /// Parse one AI Search source through the same isolated, resource-limited
    /// parser child without requiring a tenant version identity.
    pub async fn parse_for_ai_search(
        &self,
        account: AccountId,
        filename: &str,
        declared_content_type: &str,
        body: Vec<u8>,
    ) -> Result<ParseSuccess, PlatformError> {
        let deadline = Instant::now() + Duration::from_millis(self.config.request_timeout_ms);
        let account_semaphore = {
            let mut accounts = self.accounts.lock().map_err(|_| unavailable())?;
            accounts.retain(|_, semaphore| semaphore.strong_count() > 0);
            accounts
                .get(&account)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let semaphore = Arc::new(Semaphore::new(
                        self.config.max_concurrency_per_account as usize,
                    ));
                    accounts.insert(account, Arc::downgrade(&semaphore));
                    semaphore
                })
        };
        let _account = account_semaphore
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let _global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let request = ParseRequest {
            header: InputHeader {
                request_id: Uuid::now_v7().to_string(),
                filename: filename.to_owned(),
                declared_content_type: declared_content_type.to_owned(),
                content_sha256: hex::encode(Sha256::digest(&body)),
                parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
                max_input_bytes: self.config.max_input_bytes,
                tessdata_path: Some(self.tessdata_path.to_string_lossy().into_owned()),
                vision_candidate_limit: self
                    .vlm_contract
                    .as_ref()
                    .map_or(0, |contract| contract.max_images_per_document),
                html_options: None,
            },
            body,
        };
        let frame = encode_input_frame(&request).map_err(|_| protocol())?;
        let output = run_parser_child(
            &self.executable,
            frame,
            remaining(deadline)
                .map_err(|code| PlatformError::new(code, "AI Search document parsing failed"))?,
            usize::try_from(self.config.max_stderr_bytes).unwrap_or(64 * 1024),
            self.config.max_address_space_bytes,
            self.config.max_cpu_seconds,
        )
        .await
        .map_err(|code| PlatformError::new(code, "AI Search document parsing failed"))?;
        let parsed = match decode_output_frame(&output).map_err(|_| protocol())? {
            ParseOutput::Success(success)
                if success.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                *success
            }
            ParseOutput::Error(failure)
                if failure.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                return Err(PlatformError::new(
                    map_document_code(failure.error.code),
                    "AI Search document parsing failed",
                ));
            }
            _ => return Err(protocol()),
        };
        self.apply_vlm(parsed, "en", deadline)
            .await
            .map_err(|code| PlatformError::new(code, "AI Search image description failed"))
    }

    /// Dispatch one generation-authenticated Markdown Conversion operation.
    pub async fn handle(&self, request: Request) -> Response {
        match self.handle_result(request).await {
            Ok(response) => response,
            Err(error) => document_error(&error),
        }
    }

    async fn handle_result(&self, request: Request) -> Result<Response, PlatformError> {
        let authority = self.authorize(request.headers())?;
        match (request.method(), request.uri().path()) {
            (&Method::GET, "/internal/ai/to-markdown/v1/supported") => self.supported(),
            (&Method::POST, "/internal/ai/to-markdown/v1/transform") => {
                self.transform(authority, request).await
            }
            _ => Err(protocol()),
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<ParserAuthority, PlatformError> {
        let account = parse_header::<AccountId>(headers, ACCOUNT_HEADER)?;
        let worker = parse_header::<WorkerId>(headers, WORKER_HEADER)?;
        let version = parse_header::<VersionId>(headers, VERSION_HEADER)?;
        let digest = hex::decode(text_header(headers, DESCRIPTOR_HEADER)?)
            .ok()
            .and_then(|value| <[u8; 32]>::try_from(value).ok())
            .ok_or_else(protocol)?;
        let record =
            WorkerRepository::new(self.storage.db()).get_version(account, worker, version)?;
        if record.state != VersionState::Ready || record.deleted_at_ms.is_some() {
            return Err(protocol());
        }
        let (_, bindings) = version_runtime_features(self.storage.db(), version)?;
        if !bindings.iter().any(|binding| {
            binding.kind == BuiltinBindingKind::Ai && binding.descriptor_sha256 == digest
        }) {
            return Err(protocol());
        }
        Ok(ParserAuthority { account, version })
    }

    fn supported(&self) -> Result<Response, PlatformError> {
        let result = markdown_conversion_formats()
            .into_iter()
            .map(|format| SupportedResponse {
                extension: format!(".{}", format.extension),
                mime_type: format.canonical_mime,
            })
            .collect::<Vec<_>>();
        json_response(&ResponseEnvelope {
            schema_version: 1,
            result,
        })
    }

    async fn transform(
        &self,
        authority: ParserAuthority,
        request: Request,
    ) -> Result<Response, PlatformError> {
        let deadline = Instant::now() + Duration::from_millis(self.config.request_timeout_ms);
        let encoded_limit = usize::try_from(
            self.config
                .max_batch_bytes
                .saturating_mul(4)
                .saturating_div(3)
                .saturating_add(256 * 1024),
        )
        .map_err(|_| limit())?;
        let bytes = tokio::time::timeout_at(deadline, to_bytes(request.into_body(), encoded_limit))
            .await
            .map_err(|_| timeout())?
            .map_err(|_| limit())?;
        let payload: TransformRequest = serde_json::from_slice(&bytes).map_err(|_| protocol())?;
        payload.options.validate()?;
        if payload.schema_version != 1
            || payload.files.len() > usize::from(self.config.max_batch_files)
        {
            return Err(limit());
        }
        let mut decoded = Vec::with_capacity(payload.files.len());
        let mut total = 0_u64;
        for file in payload.files {
            validate_logical_name(&file.name)?;
            validate_mime(&file.mime_type)?;
            let estimate = file.data_base64.len().saturating_mul(3).saturating_div(4);
            if estimate > usize::try_from(self.config.max_input_bytes).map_err(|_| limit())? {
                return Err(limit());
            }
            let body = base64::engine::general_purpose::STANDARD
                .decode(file.data_base64)
                .map_err(|_| input())?;
            let body_len = u64::try_from(body.len()).map_err(|_| limit())?;
            if body.is_empty() || body_len > self.config.max_input_bytes {
                return Err(limit());
            }
            total = total.checked_add(body_len).ok_or_else(limit)?;
            if total > self.config.max_batch_bytes {
                return Err(limit());
            }
            decoded.push((file.name, file.mime_type, body));
        }
        let mut result = Vec::with_capacity(decoded.len());
        let mut result_bytes = 0_u64;
        for (name, mime_type, body) in decoded {
            let response = self
                .convert_one(authority, name, mime_type, body, &payload.options, deadline)
                .await;
            let response_bytes = serde_json::to_vec(&response).map_err(|_| protocol())?.len();
            result_bytes = result_bytes
                .checked_add(u64::try_from(response_bytes).map_err(|_| limit())?)
                .ok_or_else(limit)?;
            if result_bytes > self.config.max_batch_bytes {
                return Err(limit());
            }
            result.push(response);
        }
        json_response(&ResponseEnvelope {
            schema_version: 1,
            result,
        })
    }

    async fn convert_one(
        &self,
        authority: ParserAuthority,
        name: String,
        declared_mime: String,
        body: Vec<u8>,
        options: &ConversionOptions,
        deadline: Instant,
    ) -> ConversionResponse {
        let id = Uuid::now_v7().to_string();
        let html_options = options.html_options(&declared_mime);
        let parsed = match self
            .parse_child(
                authority,
                &name,
                &declared_mime,
                body,
                html_options,
                deadline,
            )
            .await
        {
            Ok(success) => {
                self.apply_vlm(success, options.description_language(), deadline)
                    .await
            }
            Err(code) => Err(code),
        };
        match parsed {
            Ok(success)
                if u64::try_from(success.markdown.len())
                    .is_ok_and(|length| length <= self.config.max_output_bytes) =>
            {
                let mut markdown = success.markdown;
                if success.format == open_compute_document_parser::DocumentFormat::Pdf
                    && options
                        .pdf
                        .as_ref()
                        .is_none_or(|pdf| pdf.metadata.unwrap_or(true))
                {
                    markdown = markdown_with_metadata(&success.metadata, &markdown);
                }
                let output_format = options
                    .output
                    .as_ref()
                    .and_then(|output| output.format)
                    .unwrap_or(OutputFormat::Markdown);
                let data = match output_format {
                    OutputFormat::Markdown => markdown,
                    OutputFormat::Text => markdown_to_text(&markdown),
                };
                if !u64::try_from(data.len())
                    .is_ok_and(|length| length <= self.config.max_output_bytes)
                {
                    return ConversionResponse::Error {
                        id,
                        name,
                        mime_type: declared_mime,
                        format: ErrorFormat::Error,
                        error: ErrorCode::DocumentLimitExceeded.as_str().to_owned(),
                    };
                }
                ConversionResponse::Success {
                    id,
                    name,
                    mime_type: success.detected_content_type,
                    format: output_format,
                    tokens: estimate_tokens(&data),
                    data,
                }
            }
            Ok(_) => ConversionResponse::Error {
                id,
                name,
                mime_type: declared_mime,
                format: ErrorFormat::Error,
                error: ErrorCode::DocumentLimitExceeded.as_str().to_owned(),
            },
            Err(error) => ConversionResponse::Error {
                id,
                name,
                mime_type: declared_mime,
                format: ErrorFormat::Error,
                error: error.as_str().to_owned(),
            },
        }
    }

    async fn parse_child(
        &self,
        authority: ParserAuthority,
        filename: &str,
        declared_content_type: &str,
        body: Vec<u8>,
        html_options: Option<HtmlConversionOptions>,
        deadline: Instant,
    ) -> Result<ParseSuccess, ErrorCode> {
        let account_semaphore = {
            let mut accounts = self
                .accounts
                .lock()
                .map_err(|_| ErrorCode::DocumentUnavailable)?;
            accounts.retain(|_, semaphore| semaphore.strong_count() > 0);
            accounts
                .get(&authority.account)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let semaphore = Arc::new(Semaphore::new(
                        self.config.max_concurrency_per_account as usize,
                    ));
                    accounts.insert(authority.account, Arc::downgrade(&semaphore));
                    semaphore
                })
        };
        let version_semaphore = {
            let mut versions = self
                .versions
                .lock()
                .map_err(|_| ErrorCode::DocumentUnavailable)?;
            versions.retain(|_, semaphore| semaphore.strong_count() > 0);
            versions
                .get(&authority.version)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| {
                    let semaphore = Arc::new(Semaphore::new(
                        self.config.max_concurrency_per_version as usize,
                    ));
                    versions.insert(authority.version, Arc::downgrade(&semaphore));
                    semaphore
                })
        };
        let _version = version_semaphore
            .try_acquire_owned()
            .map_err(|_| ErrorCode::DocumentUnavailable)?;
        let _account = account_semaphore
            .try_acquire_owned()
            .map_err(|_| ErrorCode::DocumentUnavailable)?;
        let _global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| ErrorCode::DocumentUnavailable)?;
        let request = ParseRequest {
            header: InputHeader {
                request_id: Uuid::now_v7().to_string(),
                filename: filename.to_owned(),
                declared_content_type: declared_content_type.to_owned(),
                content_sha256: hex::encode(Sha256::digest(&body)),
                parser_contract_sha256: PARSER_CONTRACT_SHA256.to_owned(),
                max_input_bytes: self.config.max_input_bytes,
                tessdata_path: Some(self.tessdata_path.to_string_lossy().into_owned()),
                vision_candidate_limit: self
                    .vlm_contract
                    .as_ref()
                    .map_or(0, |contract| contract.max_images_per_document),
                html_options,
            },
            body,
        };
        let frame = encode_input_frame(&request).map_err(map_parser_protocol)?;
        let output = run_parser_child(
            &self.executable,
            frame,
            remaining(deadline)?,
            usize::try_from(self.config.max_stderr_bytes).unwrap_or(64 * 1024),
            self.config.max_address_space_bytes,
            self.config.max_cpu_seconds,
        )
        .await?;
        match decode_output_frame(&output).map_err(map_parser_protocol)? {
            ParseOutput::Success(success)
                if success.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                Ok(*success)
            }
            ParseOutput::Success(_) => Err(ErrorCode::DocumentProtocolError),
            ParseOutput::Error(failure)
                if failure.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                Err(map_document_code(failure.error.code))
            }
            ParseOutput::Error(_) => Err(ErrorCode::DocumentProtocolError),
        }
    }

    async fn apply_vlm(
        &self,
        mut success: ParseSuccess,
        language: &str,
        deadline: Instant,
    ) -> Result<ParseSuccess, ErrorCode> {
        let candidates = std::mem::take(&mut success.vision_candidates);
        if candidates.is_empty() {
            return Ok(success);
        }
        let Some(client) = &self.vlm else {
            return Ok(success);
        };
        let contract = self
            .vlm_contract
            .as_ref()
            .ok_or(ErrorCode::DocumentUnavailable)?;
        if candidates.len() > usize::from(contract.max_images_per_document) {
            return Err(ErrorCode::DocumentProtocolError);
        }
        let mut descriptions = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            remaining(deadline)?;
            let candidate = fit_vision_candidate(candidate, contract)?;
            let _permit = self
                .vlm_semaphore
                .clone()
                .try_acquire_owned()
                .map_err(|_| ErrorCode::DocumentUnavailable)?;
            let description = tokio::time::timeout_at(
                deadline,
                client.describe(&candidate.data_base64, language),
            )
            .await
            .map_err(|_| ErrorCode::DocumentTimeout)?
            .map_err(|_| ErrorCode::DocumentParseFailed)?;
            descriptions.push((candidate.source_page, description));
        }
        success.markdown = merge_image_markdown(&descriptions, &success.markdown);
        if !u64::try_from(success.markdown.len())
            .is_ok_and(|length| length <= self.config.max_output_bytes)
        {
            return Err(ErrorCode::DocumentLimitExceeded);
        }
        success.markdown_sha256 = hex::encode(Sha256::digest(success.markdown.as_bytes()));
        success.vision_candidates.clear();
        Ok(success)
    }
}

fn fit_vision_candidate(
    candidate: VisionCandidate,
    contract: &ResolvedVlmModelContract,
) -> Result<VisionCandidate, ErrorCode> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&candidate.data_base64)
        .map_err(|_| ErrorCode::DocumentProtocolError)?;
    if hex::encode(Sha256::digest(&bytes)) != candidate.sha256 {
        return Err(ErrorCode::DocumentProtocolError);
    }
    let dimensions_fit = candidate.width <= contract.max_input_width
        && candidate.height <= contract.max_input_height
        && u64::from(candidate.width).saturating_mul(u64::from(candidate.height))
            <= contract.max_input_pixels;
    if dimensions_fit && encoded_image_fits_request(bytes.len(), contract) {
        return Ok(candidate);
    }
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|_| ErrorCode::DocumentProtocolError)?;
    let width = image.width();
    let height = image.height();
    let pixel_scale =
        (contract.max_input_pixels as f64 / f64::from(width) / f64::from(height)).sqrt();
    let scale = 1_f64
        .min(f64::from(contract.max_input_width) / f64::from(width))
        .min(f64::from(contract.max_input_height) / f64::from(height))
        .min(pixel_scale);
    let mut target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let mut target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    loop {
        let resized = image.resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        );
        let rgb = resized.to_rgb8();
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90)
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|_| ErrorCode::DocumentInputInvalid)?;
        if encoded_image_fits_request(encoded.len(), contract) {
            return Ok(VisionCandidate {
                data_base64: base64::engine::general_purpose::STANDARD.encode(&encoded),
                mime_type: "image/jpeg".to_owned(),
                width: target_width,
                height: target_height,
                sha256: hex::encode(Sha256::digest(&encoded)),
                source_page: candidate.source_page,
                ocr_performed: candidate.ocr_performed,
                ocr_confidence_milli: candidate.ocr_confidence_milli,
            });
        }
        if target_width == 1 && target_height == 1 {
            return Err(ErrorCode::DocumentVisionInputTooLarge);
        }
        target_width = (target_width.saturating_mul(3) / 4).max(1);
        target_height = (target_height.saturating_mul(3) / 4).max(1);
    }
}

fn encoded_image_fits_request(encoded_bytes: usize, contract: &ResolvedVlmModelContract) -> bool {
    let base64_bytes = encoded_bytes.div_ceil(3).saturating_mul(4);
    u64::try_from(encoded_bytes).is_ok_and(|size| size <= contract.max_encoded_image_bytes)
        && u64::try_from(base64_bytes.saturating_add(2_048))
            .is_ok_and(|size| size <= contract.max_request_bytes)
}

fn merge_image_markdown(descriptions: &[(Option<u32>, String)], ocr: &str) -> String {
    let mut sections = descriptions
        .iter()
        .map(|(page, description)| match page {
            Some(page) => format!("## Page {page} image description\n\n{}", description.trim()),
            None => format!("## Image description\n\n{}", description.trim()),
        })
        .collect::<Vec<_>>();
    if !ocr.trim().is_empty() {
        sections.push(format!("## Extracted text\n\n{}", ocr.trim()));
    }
    format!("{}\n", sections.join("\n\n"))
}

fn remaining(deadline: Instant) -> Result<Duration, ErrorCode> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(ErrorCode::DocumentTimeout)
    } else {
        Ok(remaining)
    }
}

#[cfg(test)]
#[path = "document_parser_backend_tests.rs"]
mod tests;
