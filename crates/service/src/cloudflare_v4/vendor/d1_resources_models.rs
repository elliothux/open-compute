#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    #[schemars(extend("minLength" = 1, "maxLength" = 128, "pattern" = "^[A-Za-z0-9][A-Za-z0-9_-]*$"))]
    name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct RenameResult {
    id: String,
    name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct CheckpointResult {
    #[schemars(extend("items" = {"type": "integer", "minimum": 0}))]
    checkpoints_ms: Vec<i64>,
}
