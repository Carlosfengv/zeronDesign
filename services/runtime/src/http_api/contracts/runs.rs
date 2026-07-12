use crate::types::{AgentPhase, ContentSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunRequest {
    pub project_id: String,
    pub phase: AgentPhase,
    pub agent_profile: String,
    #[serde(default)]
    pub input_context: StartRunInputContext,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunInputContext {
    #[serde(default)]
    pub content_sources: Vec<ContentSource>,
    pub brief_id: Option<String>,
    pub base_version_id: Option<String>,
    pub sandbox_binding_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub design_profile_id: Option<String>,
    pub design_fidelity_mode: Option<String>,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StartRunResponse {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueRunRequest {
    pub user_message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvePermissionRequest {
    pub decision: PermissionDecision,
    pub updated_input: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: String,
}
