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
pub struct ProjectHistoryListResponse {
    pub project_id: String,
    pub items: Vec<HistoryItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateVisualArtifactRequest {
    pub content_base64: String,
    pub client_sha256: Option<String>,
    #[serde(default)]
    pub origin_metadata: std::collections::BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct VisualArtifactResponse {
    pub artifact: crate::visual_contracts::VisualArtifact,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRunVisualBindingRequest {
    pub artifact_id: String,
    pub role: crate::visual_contracts::RunVisualBindingRole,
    pub route: String,
    pub viewport: crate::visual_contracts::VisualViewport,
    pub target: crate::visual_contracts::RunVisualTarget,
    pub order: u32,
}

#[derive(Debug, Serialize)]
pub struct RunVisualBindingResponse {
    pub binding: crate::visual_contracts::RunVisualBinding,
}

#[derive(Debug, Serialize)]
pub struct RunVisualBindingListResponse {
    pub bindings: Vec<crate::visual_contracts::RunVisualBinding>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleVisualReviewHttpRequest {
    pub mode: crate::visual_contracts::VisualReviewMode,
    pub target: crate::visual_contracts::RunVisualTarget,
    pub model: Option<String>,
    pub bindings: Vec<crate::visual_review::VisualReviewBindingInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftPreviewHeartbeatRequest {
    pub writer_lease_id: String,
    pub session_epoch: u64,
    #[serde(default = "default_draft_writer_ttl")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftPreviewTakeoverRequest {
    pub expected_session_epoch: u64,
    #[serde(default = "default_draft_writer_ttl")]
    pub ttl_seconds: u64,
}

fn default_draft_writer_ttl() -> u64 {
    120
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateElementObservationRequest {
    pub session_id: String,
    pub session_epoch: u64,
    pub workspace_revision: u64,
    pub route: String,
    pub viewport: crate::visual_contracts::VisualViewport,
    pub dom_path: String,
    pub data_slot: Option<String>,
    pub accessible_name: Option<String>,
    pub visible_text_hash: Option<String>,
    pub bounding_box: crate::visual_contracts::ElementBoundingBox,
    #[serde(default)]
    pub source_candidates: Vec<crate::visual_contracts::ElementSourceCandidate>,
    pub screenshot_crop_artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateEditImpactPlanRequest {
    pub observation_id: Option<String>,
    pub predecessor_run_id: Option<String>,
    pub scope: crate::visual_contracts::EditImpactScope,
    pub targets: Vec<String>,
    pub operations: Vec<crate::visual_contracts::EditImpactOperation>,
    pub risk: crate::visual_contracts::EditImpactRisk,
    pub edit_base: crate::visual_contracts::EditBase,
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
    pub model_service_id: Option<String>,
    pub model_service_display_name: Option<String>,
    pub style_contract_path: Option<String>,
    pub style_contract: Option<Value>,
    pub latest_build: Option<Value>,
    pub dependency_state: Option<Value>,
    pub preview: Option<Value>,
}
