use super::super::*;

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
