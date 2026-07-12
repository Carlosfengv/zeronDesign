use super::super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishWorkRequest {
    pub release_id: String,
    #[serde(default)]
    pub expected_current_release_id: Option<String>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
    #[serde(default = "default_static_web_profile")]
    pub runtime_profile_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnpublishWorkRequest {
    #[serde(default)]
    pub expected_current_release_id: Option<String>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
    #[serde(default = "default_static_web_profile")]
    pub runtime_profile_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationOperationResponse {
    pub operation: crate::publication::PublishOperation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentStateResponse {
    pub runtime: crate::publication::WorkRuntimeState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkReleaseListResponse {
    pub project_id: String,
    pub releases: Vec<crate::release::WorkRelease>,
}

fn default_static_web_profile() -> String {
    crate::release::STATIC_WEB_PROFILE_ID.to_string()
}
