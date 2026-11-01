#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RestoreRequest {
    #[schemars(extend("minLength" = 1, "maxLength" = 128, "pattern" = "^[^\\u0000-\\u001F\\u007F]+$"))]
    name: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct RestoredResource {
    id: String,
    #[schemars(extend("minLength" = 1, "maxLength" = 128))]
    name: String,
    #[schemars(extend("enum" = ["kv_namespace","d1_database"]))]
    kind: &'static str,
    #[schemars(extend("format" = "date-time"))]
    created_on: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct Backup {
    id: String,
    #[schemars(extend("format" = "date-time"))]
    created_on: String,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("minimum" = 0, "type" = "integer"))]
    size: Option<u64>,
}
