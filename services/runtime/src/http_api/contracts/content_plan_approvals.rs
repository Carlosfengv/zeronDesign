use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordContentPlanApprovalRequest {
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub confirmation_event_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordContentPlanChangeRequest {
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub change_event_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyContentPlanApprovalQuery {
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
}
