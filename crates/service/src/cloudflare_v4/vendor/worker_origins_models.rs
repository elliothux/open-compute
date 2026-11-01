#[derive(Serialize, schemars::JsonSchema)]
struct PublicOriginBinding {
    name: String,
    #[schemars(extend("format" = "uri"))]
    url: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PublicOriginInput {
    #[schemars(extend("maxLength" = 63, "pattern" = "^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$"))]
    name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct WorkerEndpoint {
    id: String,
    #[schemars(extend("enum" = ["local_origin","public_origin"]))]
    kind: WorkerEndpointKind,
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
enum WorkerEndpointScope {
    LocalMachine,
    PublicNetwork,
}
