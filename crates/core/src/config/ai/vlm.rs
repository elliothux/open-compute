//! Operator-pinned image-description model contracts.

use super::*;

/// Frozen operator mapping for an image-description model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiVlmModelConfig {
    /// Operation-specific chat-completions backend entry name.
    pub backend: String,
    /// Model value sent to the provider.
    pub remote_model: String,
    /// Optional immutable provider revision.
    #[serde(default)]
    pub provider_revision: Option<String>,
    /// Maximum raster width accepted by the model.
    pub max_input_width: u32,
    /// Maximum raster height accepted by the model.
    pub max_input_height: u32,
    /// Maximum decoded raster pixels accepted by the model.
    pub max_input_pixels: u64,
    /// Maximum JPEG bytes accepted by the model.
    pub max_encoded_image_bytes: u64,
    /// Maximum response tokens requested from the model.
    pub max_output_tokens: u32,
}

impl AiVlmModelConfig {
    pub(super) fn validate(
        &self,
        backends: &BTreeMap<String, AiBackendConfig>,
    ) -> Result<(), PlatformError> {
        let dimensions =
            u64::from(self.max_input_width).saturating_mul(u64::from(self.max_input_height));
        if !backends
            .get(&self.backend)
            .is_some_and(|backend| backend.protocol == AiBackendProtocol::OpenAiChatCompletionsV1)
            || self.max_input_width == 0
            || self.max_input_width > 8_192
            || self.max_input_height == 0
            || self.max_input_height > 8_192
            || self.max_input_pixels == 0
            || self.max_input_pixels > 16_777_216
            || self.max_input_pixels > dimensions
            || self.max_encoded_image_bytes == 0
            || self.max_encoded_image_bytes > 16 * 1024 * 1024
            || self.max_output_tokens == 0
            || self.max_output_tokens > 4_096
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI VLM model contract is invalid",
            ));
        }
        validate_nonempty(&self.remote_model, MAX_MODEL_NAME_BYTES)?;
        validate_optional_nonempty(self.provider_revision.as_deref(), MAX_REVISION_BYTES)
    }
}

/// Secret-free immutable VLM model and image-preprocessing contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedVlmModelContract {
    /// Operator-visible model alias.
    pub alias: String,
    /// Operator backend entry name.
    pub backend_name: String,
    /// Fixed backend protocol token.
    pub protocol: String,
    /// Digest of the canonical endpoint URL.
    pub endpoint_sha256: String,
    /// Secret-free authentication kind.
    pub auth_kind: String,
    /// Custom authentication header name, when configured.
    pub auth_header_name: Option<String>,
    /// Digest of static request headers.
    pub headers_sha256: String,
    /// Provider model value.
    pub remote_model: String,
    /// Optional provider-supported immutable revision.
    pub provider_revision: Option<String>,
    /// Maximum raster width.
    pub max_input_width: u32,
    /// Maximum raster height.
    pub max_input_height: u32,
    /// Maximum raster pixels.
    pub max_input_pixels: u64,
    /// Maximum encoded JPEG bytes.
    pub max_encoded_image_bytes: u64,
    /// Maximum requested output tokens.
    pub max_output_tokens: u32,
    /// Maximum page/image candidates described for one document.
    pub max_images_per_document: u16,
    /// Maximum serialized request bytes.
    pub max_request_bytes: u64,
    /// Maximum serialized response bytes.
    pub max_response_bytes: u64,
    /// Fixed image-description prompt revision.
    pub prompt_revision: String,
    /// Fixed local image-preprocessing revision.
    pub image_preprocessing_revision: String,
    /// Digest of this complete contract with this field empty.
    pub contract_sha256: String,
}
