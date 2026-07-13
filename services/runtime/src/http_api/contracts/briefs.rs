use crate::types::{AgentRunStatus, Brief, BriefStatus};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefResponse {
    pub brief_id: String,
    pub project_id: String,
    pub run_id: String,
    pub status: BriefStatus,
    pub run_status: AgentRunStatus,
    pub brief: Brief,
}
