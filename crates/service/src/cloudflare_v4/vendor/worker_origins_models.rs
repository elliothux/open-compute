#[derive(Serialize, schemars::JsonSchema)]
struct PublicOriginBinding {
    id: String,
    #[schemars(extend("enum" = ["managed_gateway","external_proxy"]))]
    transport: PublicOriginTransport,
    /// Managed Gateway label under the instance base domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Public origin without a trailing slash or default port.
    #[schemars(extend("format" = "uri"))]
    origin: String,
    /// Whether the OCD side of this transport can serve the origin now.
    ingress_available: bool,
    #[schemars(extend("format" = "date-time"))]
    created_on: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
enum PublicOriginInput {
    ManagedGateway {
        #[schemars(extend("maxLength" = 63, "pattern" = "^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$"))]
        name: String,
    },
    ExternalProxy {
        /// `http(s)://hostname[:port]` with an operator-authorized exact hostname.
        #[schemars(extend("maxLength" = 268, "format" = "uri"))]
        origin: String,
    },
}

#[derive(Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum PublicOriginTransport {
    ManagedGateway,
    ExternalProxy,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct WorkerEndpoint {
    id: String,
    #[schemars(extend("enum" = ["local_origin","public_origin"]))]
    kind: WorkerEndpointKind,
    #[schemars(extend("enum" = ["local_http","managed_gateway","external_proxy"]))]
    transport: WorkerEndpointTransport,
    #[schemars(extend("format" = "uri"))]
    url: String,
    #[schemars(extend("enum" = ["local_machine","public_network"]))]
    scope: WorkerEndpointScope,
    #[schemars(extend("format" = "date-time"))]
    created_on: String,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkerEndpointKind {
    LocalOrigin,
    PublicOrigin,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkerEndpointTransport {
    LocalHttp,
    ManagedGateway,
    ExternalProxy,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkerEndpointScope {
    LocalMachine,
    PublicNetwork,
}
