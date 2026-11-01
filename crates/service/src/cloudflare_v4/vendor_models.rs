const CF_VERSION: &str = "1.0.0-beta.12";

#[derive(Serialize, schemars::JsonSchema)]
struct Capabilities<'a> {
    #[schemars(extend("minLength" = 1))]
    release: &'a str,
    #[schemars(extend("const" = CF_VERSION))]
    cf_version: &'a str,
    compatibility: open_compute_core::RuntimeCompatibilityV1,
    system_workers: open_compute_core::SystemWorkerCompatibilityV1,
    #[schemars(extend("additionalProperties" = {"type":"string","enum":["supported","supported_with_deviation","unsupported"]}))]
    endpoints: BTreeMap<&'a str, &'static str>,
    #[schemars(extend("uniqueItems" = true))]
    deviations: Vec<String>,
    limits: BTreeMap<String, u64>,
    configuration: ProductConfiguration,
}

#[derive(Serialize, schemars::JsonSchema)]
struct ProductConfiguration {
    ai_search: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
struct SystemStatus {
    state: &'static str,
    version: &'static str,
    components: Vec<StatusComponent>,
    operator_proxy: OperatorProxyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "object"))]
    observability: Option<ObservabilityStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "object"))]
    deployment_runtime:
        Option<open_compute_storage::worker_repository::DeploymentRuntimeAssessmentSummary>,
}

#[derive(Serialize, schemars::JsonSchema)]
struct OperatorProxyStatus {
    #[schemars(extend("enum" = ["direct","proxy","invalid"]))]
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    source_variable: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    origin: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
struct ObservabilityStatus {
    #[schemars(extend("enum" = ["healthy","degraded"]))]
    state: &'static str,
    retention_ms: u64,
    max_database_bytes: u64,
    query_max_timeframe_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("minimum" = 0, "type" = "integer"))]
    oldest_event_ms: Option<i64>,
    tail_sessions: usize,
    closed_client_drops: u64,
    overload_drops: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
struct StatusComponent {
    name: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    message: Option<&'static str>,
}

#[derive(Serialize, schemars::JsonSchema)]
struct SchedulerStatus {
    state: &'static str,
    pending: u64,
    running: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
struct CacheStatus {
    entries: u64,
    bytes: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
struct ImageCapacity {
    queued: u64,
    running: u64,
    capacity: u16,
}
