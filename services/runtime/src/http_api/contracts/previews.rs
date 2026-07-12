use super::super::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCurrentResponse {
    pub project_id: String,
    pub version_id: String,
    pub preview_url: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewVersionResponse {
    pub project_id: String,
    pub version_id: String,
    pub preview_url: String,
    pub status: String,
}
