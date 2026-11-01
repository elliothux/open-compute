#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct MigrationInput {
    #[schemars(extend("minimum" = 1))]
    id: u32,
    #[schemars(extend("minLength" = 1, "maxLength" = 255))]
    name: String,
    #[schemars(extend("pattern" = "^[0-9a-f]{64}$"))]
    sha256: String,
    #[schemars(extend("minLength" = 1))]
    sql: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct MigrationOutput {
    #[schemars(extend("minimum" = 1))]
    id: u32,
    #[schemars(extend("minLength" = 1, "maxLength" = 255))]
    name: String,
    #[schemars(extend("pattern" = "^[0-9a-f]{64}$"))]
    sha256: String,
    #[schemars(extend("minimum" = 0))]
    applied_at_ms: i64,
}
