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
    AccountId, DocumentParserConfig, ErrorCode, PlatformError, VersionId, WorkerId,
};
use open_compute_document_parser::{
    HtmlConversionOptions, InputHeader, PARSER_CONTRACT_SHA256, ParseOutput, ParseRequest,
    ParseSuccess, decode_output_frame, encode_input_frame, supported_formats,
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

/// One version-scoped Markdown Conversion service.
pub struct DocumentParserBindingService {
    storage: Arc<PlatformStorage>,
    config: DocumentParserConfig,
    executable: PathBuf,
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
    /// Compose the binding service with the running `ocd` executable.
    pub fn new(
        storage: Arc<PlatformStorage>,
        config: DocumentParserConfig,
    ) -> Result<Self, PlatformError> {
        let executable = std::env::current_exe().map_err(|_| unavailable())?;
        if !executable.is_absolute() {
            return Err(unavailable());
        }
        Ok(Self::with_executable(storage, config, executable))
    }

    /// Compose a service with an explicit executable, primarily for real-process fixtures.
    #[must_use]
    pub fn with_executable(
        storage: Arc<PlatformStorage>,
        config: DocumentParserConfig,
        executable: PathBuf,
    ) -> Self {
        Self {
            storage,
            global: Arc::new(Semaphore::new(config.max_concurrency as usize)),
            accounts: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
            executable,
            config,
        }
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
        match decode_output_frame(&output).map_err(|_| protocol())? {
            ParseOutput::Success(success)
                if success.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                Ok(success)
            }
            ParseOutput::Error(failure)
                if failure.parser_contract_sha256 == PARSER_CONTRACT_SHA256 =>
            {
                Err(PlatformError::new(
                    map_document_code(failure.error.code),
                    "AI Search document parsing failed",
                ))
            }
            _ => Err(protocol()),
        }
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
        let result = supported_formats()
            .iter()
            .map(|format| SupportedResponse {
                extension: format!(".{}", format.extension),
                mime_type: format.mime_type,
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
        let parsed = self
            .parse_child(
                authority,
                &name,
                &declared_mime,
                body,
                html_options,
                deadline,
            )
            .await;
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
                Ok(success)
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
