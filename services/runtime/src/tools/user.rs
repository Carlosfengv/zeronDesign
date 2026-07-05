use crate::{
    conversation::RuntimeStore,
    types::{AgentEvent, AgentRunStatus},
};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};

pub async fn ask(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    input: &Value,
) -> Result<Value> {
    let text = input
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("More information is needed.");
    store
        .append_conversation_item(
            project_id,
            Some(run_id),
            "approval_request",
            Some("assistant"),
            text,
            None,
        )
        .await;
    store
        .append_event(AgentEvent::StateChanged {
            run_id: run_id.to_string(),
            state: "needs_user_input".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run_id.to_string(),
            text: text.to_string(),
            timestamp: Utc::now(),
        })
        .await;
    store
        .update_run_status(run_id, AgentRunStatus::NeedsUserInput)
        .await?;
    Ok(json!({ "status": "needs_user_input", "message": text }))
}
