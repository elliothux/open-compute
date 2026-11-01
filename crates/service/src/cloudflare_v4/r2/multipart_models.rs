#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateBody {
    #[schemars(extend("minLength" = 1))]
    key: String,
    #[serde(default)]
    options: CreateOptions,
}

#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateOptions {
    #[serde(default)]
    http_metadata: R2HttpMetadata,
    #[serde(default)]
    custom_metadata: BTreeMap<String, String>,
    #[schemars(extend("type" = "string", "enum" = ["Standard", "InfrequentAccess"]))]
    storage_class: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CompleteBody {
    #[schemars(extend("minItems" = 1, "maxItems" = 10000, "items" = {"$ref": "#/components/schemas/R2MultipartPart"}))]
    parts: Vec<R2UploadedPart>,
}
