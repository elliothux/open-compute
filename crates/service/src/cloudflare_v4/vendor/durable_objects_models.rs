#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct DurableObjectNamespace {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) script_name: String,
    pub(super) class_name: String,
    #[schemars(extend("enum" = ["creating","ready","deleting","tombstoned"]))]
    pub(super) state: &'static str,
    #[schemars(extend("enum" = ["healthy","degraded","unavailable"]))]
    pub(super) availability: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    pub(super) availability_code: Option<String>,
    #[schemars(extend("minimum" = 0))]
    pub(super) spec_generation: u64,
    #[schemars(extend("minimum" = 1))]
    pub(super) schema_version: u32,
    #[schemars(extend("format" = "date-time"))]
    pub(super) created_on: String,
    #[schemars(extend("format" = "date-time"))]
    pub(super) modified_on: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct DurableObjectRecord {
    pub(super) id: String,
    pub(super) namespace_id: String,
    #[schemars(extend("minimum" = 1))]
    pub(super) generation: u64,
    pub(super) state: &'static str,
    #[schemars(extend("format" = "date-time"))]
    pub(super) created_on: String,
    #[schemars(extend("format" = "date-time"))]
    pub(super) modified_on: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("format" = "date-time", "type" = "string"))]
    pub(super) deleted_on: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct DurableObjectNamespacePage {
    #[schemars(extend("items" = {"$ref":"#/components/schemas/DurableObjectNamespace"}))]
    pub(super) items: Vec<DurableObjectNamespace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    pub(super) next_cursor: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct DurableObjectRecordPage {
    #[schemars(extend("items" = {"$ref":"#/components/schemas/DurableObjectRecord"}))]
    pub(super) items: Vec<DurableObjectRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("type" = "string"))]
    pub(super) next_cursor: Option<String>,
}
