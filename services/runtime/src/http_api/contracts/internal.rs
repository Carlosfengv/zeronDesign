use super::super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotePreviewRequest {
    pub project_id: String,
    pub run_id: String,
    pub candidate_version_id: String,
    #[serde(default)]
    pub gate_report: PromotePreviewGateReport,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalTemplateBuildRequest {
    pub project_id: String,
    pub template: String,
    pub audience: String,
    #[serde(default)]
    pub content_hierarchy: Vec<String>,
    pub visual_direction: String,
    #[serde(default)]
    pub page_structure: Value,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub missing_information: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub public_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalTemplateBuildResponse {
    pub project_id: String,
    pub brief_id: String,
    pub run_id: String,
    pub version_id: String,
    pub checkpoint_id: String,
    pub stream_url: String,
    pub preview_url: String,
    pub artifact_url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotePreviewGateReport {
    #[serde(default)]
    pub build_log_has_terminal_error: bool,
    #[serde(default = "default_true")]
    pub preview_accessible: bool,
    #[serde(default)]
    pub screenshot_blank: bool,
    #[serde(default = "default_true")]
    pub screenshot_available: bool,
    #[serde(default)]
    pub blocking_findings: u32,
}

impl From<PromotePreviewGateReport> for PromotionGateReport {
    fn from(value: PromotePreviewGateReport) -> Self {
        Self {
            build_log_has_terminal_error: value.build_log_has_terminal_error,
            preview_accessible: value.preview_accessible,
            screenshot_blank: value.screenshot_blank,
            screenshot_available: value.screenshot_available,
            blocking_findings: value.blocking_findings,
        }
    }
}

fn default_true() -> bool {
    true
}
