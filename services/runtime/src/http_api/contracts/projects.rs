use super::super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuery {
    #[serde(default)]
    pub include_debug: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationListResponse {
    pub project_id: String,
    pub items: Vec<ConversationItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntimeStateResponse {
    pub project_id: String,
    pub current_version_id: String,
    pub sandbox_binding_id: String,
    pub source_snapshot_uri: String,
    pub app_root: String,
    pub template_key: String,
    pub style_contract_path: Option<String>,
    pub style_contract: Option<Value>,
    pub latest_build: Option<Value>,
    pub dependency_state: Option<Value>,
    pub preview: Option<Value>,
}
