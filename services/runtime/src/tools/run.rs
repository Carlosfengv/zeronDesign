use crate::{conversation::RuntimeStore, types::AgentEvent};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};

pub async fn report_progress(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    input: &Value,
) -> Result<Value> {
    let text = input
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Working.");
    store
        .append_conversation_item(
            project_id,
            Some(run_id),
            "progress",
            Some("assistant"),
            text,
            None,
        )
        .await;
    let _ = store
        .append_event(AgentEvent::AgentMessage {
            run_id: run_id.to_string(),
            text: text.to_string(),
            timestamp: Utc::now(),
        })
        .await;
    Ok(json!({ "reported": true }))
}

pub async fn complete(input: &Value) -> Result<Value> {
    let status = match input.get("status").and_then(Value::as_str) {
        Some("completed") | None => "completed",
        Some("partial") => "partial",
        Some("blocked") => "blocked",
        Some("failed") => "failed",
        Some("cancelled") => "cancelled",
        Some(status) => return Err(anyhow::anyhow!("unsupported completion status: {status}")),
    };
    let summary = input
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Run completed.")
        .to_string();
    Ok(json!({ "status": status, "summary": summary }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn complete_rejects_unknown_status_instead_of_coercing_to_completed() {
        let error = complete(&json!({
            "status": "complete",
            "summary": "typo must not complete the run"
        }))
        .await
        .unwrap_err();

        assert!(error.to_string().contains("unsupported completion status"));
    }
}
