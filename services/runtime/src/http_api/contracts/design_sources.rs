use super::super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDesignSourceArtifactRequest {
    pub scope: Value,
    pub file_name: String,
    pub media_type: String,
    pub content_base64: String,
    pub client_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DesignSourceArtifactResponse {
    pub artifact: DesignSourceArtifact,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDesignProfileRequest {
    pub name: String,
    pub scope: Value,
    pub source_artifact_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDesignProfileResponse {
    pub design_profile_draft: DesignProfileDraft,
    pub conversion_report: DesignProfileConversionReport,
    pub requires_review: bool,
}
