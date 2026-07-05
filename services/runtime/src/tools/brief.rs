use crate::{
    conversation::RuntimeStore,
    types::{AgentEvent, Brief},
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Value};

pub async fn write_draft(store: &RuntimeStore, run_id: &str, input: Value) -> Result<Value> {
    let brief: Brief = serde_json::from_value(input)
        .map_err(|err| anyhow!("brief.write_draft received invalid brief JSON: {err}"))?;
    brief
        .validate_for_runtime()
        .map_err(|err| anyhow!("brief.write_draft validation failed: {err}"))?;
    let brief_id = store.write_brief_draft(run_id, brief).await?;
    Ok(json!({ "briefId": brief_id }))
}

pub async fn request_confirmation(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    message: Option<&str>,
) -> Result<Value> {
    let text = message.unwrap_or("Brief is ready for confirmation.");
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
        .append_event(AgentEvent::AgentMessage {
            run_id: run_id.to_string(),
            text: text.to_string(),
            timestamp: Utc::now(),
        })
        .await;
    Ok(json!({ "status": "needs_user_input", "message": text }))
}
