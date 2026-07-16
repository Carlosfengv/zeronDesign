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
    pub model_resource_id: Option<String>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

impl From<StartRunRequest> for crate::run_lifecycle::StartRunCommand {
    fn from(request: StartRunRequest) -> Self {
        Self {
            project_id: request.project_id,
            phase: request.phase,
            agent_profile: request.agent_profile,
            input_context: crate::run_lifecycle::StartRunContext {
                content_sources: request.input_context.content_sources,
                brief_id: request.input_context.brief_id,
                base_version_id: request.input_context.base_version_id,
                sandbox_binding_id: request.input_context.sandbox_binding_id,
                parent_run_id: request.input_context.parent_run_id,
                design_profile_id: request.input_context.design_profile_id,
                design_fidelity_mode: request.input_context.design_fidelity_mode,
                workspace_id: request.input_context.workspace_id,
                organization_id: request.input_context.organization_id,
                model_resource_id: request.input_context.model_resource_id,
                finding_ids: request.input_context.finding_ids,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StartRunResponse {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: String,
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
