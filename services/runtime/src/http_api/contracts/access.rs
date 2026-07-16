use super::super::*;
use crate::types::DesignContextEnforcementPolicy;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertProjectAccessRequest {
    pub owner_principal_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub organization_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAccessResponse {
    pub project_access: ProjectAccessRecord,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpsertDesignContextEnforcementPolicyRequest {
    pub design_profile_id: String,
    pub design_profile_version: u32,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
    pub updated_by: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextEnforcementPolicyResponse {
    pub policy: DesignContextEnforcementPolicy,
}
