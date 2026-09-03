//! Closed Cloudflare Worker request models used by the pinned Wrangler client.

use open_compute_core::SecretString;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Cloudflare Worker upload metadata emitted by Wrangler 4.127.1.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadMetadata {
    /// ES module entry point.
    pub main_module: Option<String>,
    /// Service-worker/CommonJS entry point.
    pub body_part: Option<String>,
    /// Immutable runtime compatibility date.
    pub compatibility_date: String,
    /// Immutable runtime compatibility flags.
    #[serde(default)]
    pub compatibility_flags: Vec<String>,
    /// Environment and product bindings.
    #[serde(default)]
    pub bindings: Vec<WorkerUploadBinding>,
    /// Binding kinds explicitly inherited by Wrangler.
    #[serde(default)]
    pub keep_bindings: Vec<String>,
    /// Immutable version annotations.
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Static Assets completion token and routing configuration.
    pub assets: Option<WorkerUploadAssets>,
    /// Version-scoped automatic cache configuration.
    pub cache_options: Option<WorkerUploadCacheOptions>,
    /// Declarative Durable Object and Worker entrypoint exports.
    pub exports: Option<BTreeMap<String, WorkerUploadExport>>,
    /// Declarative Durable Object migrations.
    pub migrations: Option<WorkerUploadMigrations>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadCacheOptions {
    pub enabled: bool,
    #[serde(default)]
    pub cross_version_cache: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum WorkerUploadExport {
    Worker {
        cache: Option<WorkerUploadEntrypointCache>,
    },
    DurableObject {
        state: Option<String>,
        storage: Option<String>,
        renamed_to: Option<String>,
        container: Option<String>,
        transferred_to: Option<String>,
        transfer_from: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadEntrypointCache {
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadMigrations {
    pub(super) old_tag: Option<String>,
    pub(super) new_tag: String,
    pub(super) steps: Vec<WorkerUploadMigrationStep>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadMigrationStep {
    #[serde(default)]
    pub(super) new_classes: Vec<String>,
    #[serde(default)]
    pub(super) new_sqlite_classes: Vec<String>,
    #[serde(default)]
    pub(super) renamed_classes: Vec<WorkerUploadClassRename>,
    #[serde(default)]
    pub(super) deleted_classes: Vec<String>,
    #[serde(default)]
    pub(super) transferred_classes: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadClassRename {
    pub(super) from: String,
    pub(super) to: String,
}

impl std::fmt::Debug for WorkerUploadMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerUploadMetadata")
            .field("main_module", &self.main_module)
            .field("body_part", &self.body_part)
            .field("compatibility_date", &self.compatibility_date)
            .field("compatibility_flags", &self.compatibility_flags)
            .field("bindings", &self.bindings.len())
            .field("keep_bindings", &self.keep_bindings)
            .field("annotations", &self.annotations)
            .field("has_assets", &self.assets.is_some())
            .field("has_cache_options", &self.cache_options.is_some())
            .field("has_exports", &self.exports.is_some())
            .field("has_migrations", &self.migrations.is_some())
            .finish()
    }
}

/// Static Assets token and configuration carried by a Worker upload.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadAssets {
    /// Opaque completion token produced after every manifest object is verified.
    pub jwt: String,
    /// Static Assets routing configuration.
    pub config: WorkerUploadAssetsConfig,
}

/// Supported Wrangler Static Assets configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkerUploadAssetsConfig {
    /// HTML response mode.
    pub html_handling: Option<String>,
    /// Not-found response mode.
    pub not_found_handling: Option<String>,
    /// Whether matching requests reach tenant code before the asset server.
    pub run_worker_first: Option<serde_json::Value>,
    /// Optional `_redirects` contents.
    pub _redirects: Option<String>,
    /// Optional `_headers` contents.
    pub _headers: Option<String>,
}

/// P6 binding subset emitted in Wrangler multipart metadata.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerUploadBinding {
    /// Plain UTF-8 environment value.
    PlainText { name: String, text: String },
    /// JSON environment value.
    Json {
        name: String,
        json: serde_json::Value,
    },
    /// Write-only encrypted environment value.
    SecretText { name: String, text: SecretString },
    /// Existing KV namespace.
    KvNamespace {
        name: String,
        namespace_id: String,
        raw: Option<bool>,
    },
    /// Existing R2 bucket.
    R2Bucket {
        name: String,
        bucket_name: String,
        jurisdiction: Option<String>,
        raw: Option<bool>,
    },
    /// Existing D1 database.
    D1 {
        name: String,
        id: String,
        #[serde(rename = "internalEnv")]
        internal_env: Option<String>,
        raw: Option<bool>,
    },
    /// Existing Vectorize index.
    Vectorize {
        name: String,
        index_name: String,
        raw: Option<bool>,
    },
    /// Existing AI Search namespace.
    AiSearchNamespace { name: String, namespace: String },
    /// Existing AI Search instance.
    AiSearch { name: String, instance_name: String },
    /// Platform-provided Workers AI Markdown conversion subset.
    Ai {
        name: String,
        staging: Option<bool>,
        raw: Option<bool>,
    },
    /// Existing Durable Object namespace.
    DurableObjectNamespace {
        name: String,
        class_name: String,
        script_name: Option<String>,
        environment: Option<String>,
    },
    /// Existing Queue producer.
    Queue {
        name: String,
        queue_name: String,
        delivery_delay: Option<u32>,
        raw: Option<bool>,
    },
    /// Existing Workflow definition.
    Workflow {
        name: String,
        workflow_name: String,
        class_name: Option<String>,
        script_name: Option<String>,
        raw: Option<bool>,
    },
    /// Same-account Worker service binding.
    Service {
        name: String,
        service: String,
        environment: Option<String>,
        entrypoint: Option<String>,
        cross_account_grant: Option<String>,
        props: Option<serde_json::Value>,
    },
    /// Platform-provided Images binding.
    Images { name: String },
    /// Immutable version metadata binding.
    VersionMetadata { name: String },
    /// Static Assets fetcher binding.
    Assets { name: String },
    /// Legacy service-worker WebAssembly binding backed by a multipart part.
    WasmModule { name: String, part: String },
    /// Legacy service-worker text binding backed by a multipart part.
    TextBlob { name: String, part: String },
    /// Legacy service-worker byte binding backed by a multipart part.
    DataBlob { name: String, part: String },
    /// Explicit inheritance marker emitted by Wrangler.
    Inherit { name: String },
}

impl WorkerUploadBinding {
    /// Return the tenant environment name without exposing binding values.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::PlainText { name, .. }
            | Self::Json { name, .. }
            | Self::SecretText { name, .. }
            | Self::KvNamespace { name, .. }
            | Self::R2Bucket { name, .. }
            | Self::D1 { name, .. }
            | Self::Vectorize { name, .. }
            | Self::AiSearchNamespace { name, .. }
            | Self::AiSearch { name, .. }
            | Self::Ai { name, .. }
            | Self::DurableObjectNamespace { name, .. }
            | Self::Queue { name, .. }
            | Self::Workflow { name, .. }
            | Self::Service { name, .. }
            | Self::Images { name }
            | Self::VersionMetadata { name }
            | Self::Assets { name }
            | Self::WasmModule { name, .. }
            | Self::TextBlob { name, .. }
            | Self::DataBlob { name, .. }
            | Self::Inherit { name } => name,
        }
    }

    /// Return an explicitly referenced multipart part for legacy blob bindings.
    pub(crate) fn part(&self) -> Option<(&str, open_compute_workers::ModuleType)> {
        match self {
            Self::WasmModule { part, .. } => Some((part, open_compute_workers::ModuleType::Wasm)),
            Self::TextBlob { part, .. } => Some((part, open_compute_workers::ModuleType::Text)),
            Self::DataBlob { part, .. } => Some((part, open_compute_workers::ModuleType::Data)),
            _ => None,
        }
    }

    /// Whether fixed Wrangler supplied an option outside the declared P6 subset.
    pub(crate) fn has_unsupported_options(&self) -> bool {
        match self {
            Self::KvNamespace { raw, .. } | Self::Vectorize { raw, .. } => raw.is_some(),
            Self::R2Bucket {
                jurisdiction, raw, ..
            } => jurisdiction.is_some() || raw.is_some(),
            Self::D1 {
                internal_env, raw, ..
            } => internal_env.is_some() || raw.is_some(),
            Self::Ai { staging, raw, .. } => staging.is_some() || raw.is_some(),
            Self::Queue { raw, .. } | Self::Workflow { raw, .. } => raw.is_some(),
            Self::DurableObjectNamespace { environment, .. } => environment.is_some(),
            Self::Service {
                environment,
                cross_account_grant,
                props,
                ..
            } => environment.is_some() || cross_account_grant.is_some() || props.is_some(),
            _ => false,
        }
    }
}
