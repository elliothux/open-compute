#[derive(serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MultipartCreated {
    key: String,
    upload_id: String,
}
