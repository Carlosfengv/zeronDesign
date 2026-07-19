use super::super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateDesignProfileRequest {
    pub expected_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationConflictResponse {
    pub error: String,
    pub current_version: u32,
    pub validation_issues: Vec<DesignProfileValidationIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDesignProfileRequest {
    pub project_id: Option<String>,
    pub name: String,
    pub profile: Option<Value>,
    #[serde(flatten)]
    pub legacy_profile: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDesignProfileRequest {
    pub expected_version: Option<u32>,
    pub name: String,
    pub profile: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileResponse {
    pub design_profile: DesignProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<DesignProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindProjectDesignProfileRequest {
    pub design_profile_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDesignProfileResponse {
    pub project_id: String,
    pub design_profile: Option<DesignProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<DesignProfile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDesignProfilesQuery {
    pub project_id: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDesignProfilesResponse {
    pub design_profiles: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDiffQuery {
    pub from_version: u32,
    pub to_version: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileFidelityQuery {
    pub surface: Option<String>,
    pub template: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileVersionsResponse {
    pub design_profile_id: String,
    pub versions: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDiffResponse {
    pub design_profile_id: String,
    pub from_version: u32,
    pub to_version: u32,
    pub changes: Vec<crate::design_profile_service::ProfileDiffChange>,
}
