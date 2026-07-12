use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub service: &'static str,
    pub repository_commit: String,
    pub repository_dirty: bool,
    pub image_ref: Option<String>,
}
