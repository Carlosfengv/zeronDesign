use super::{conflict, internal, RunLifecycleError, RunLifecycleOutcome, RunLifecycleService};
use crate::types::{AgentEvent, AgentRunStatus};
use chrono::Utc;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

impl PermissionDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

impl RunLifecycleService {
    pub async fn resolve_permission(
        &self,
        permission_id: &str,
        decision: PermissionDecision,
        updated_input: Option<Value>,
    ) -> Result<RunLifecycleOutcome, RunLifecycleError> {
        let pending = self
            .store
            .pending_permission(permission_id)
            .await
            .ok_or_else(|| {
                RunLifecycleError::NotFound(format!(
                    "permission request not found: {permission_id}"
                ))
            })?;
        if pending.status != "pending" {
            return Err(RunLifecycleError::Conflict(format!(
                "permission request {permission_id} is already {}",
                pending.status
            )));
        }
        let run = self.store.get_run(&pending.run_id).await.ok_or_else(|| {
            RunLifecycleError::NotFound(format!("run not found: {}", pending.run_id))
        })?;
        if run.status.is_terminal() {
            return Err(RunLifecycleError::Conflict(format!(
                "run {} is already terminal with status {:?}",
                run.id, run.status
            )));
        }
        let permission = self
            .store
            .resolve_permission_with_input(permission_id, decision.as_str(), updated_input.clone())
            .await
            .map_err(internal)?;

        let status = match decision {
            PermissionDecision::Allow => {
                self.store
                    .update_run_status(&permission.run_id, AgentRunStatus::Running)
                    .await
                    .map_err(conflict)?;
                self.store
                    .append_event(AgentEvent::StateChanged {
                        run_id: permission.run_id.clone(),
                        state: "running".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await
                    .map_err(internal)?;
                self.store
                    .append_audit_record(
                        &permission.project_id,
                        &permission.run_id,
                        &permission.tool,
                        permission_input_summary(updated_input.as_ref()),
                        "allow",
                        "permission resolved by API",
                    )
                    .await;
                self.store
                    .append_conversation_item(
                        &permission.project_id,
                        Some(&permission.run_id),
                        "permission_resolved",
                        Some("system"),
                        format!(
                            "Permission approved for {}; retry the tool call",
                            permission.tool
                        ),
                        Some(serde_json::json!({
                            "permissionId": permission.id,
                            "tool": permission.tool,
                            "toolUseId": permission.tool_use_id,
                            "decision": "allow",
                            "updatedInputProvided": updated_input.is_some(),
                        })),
                    )
                    .await;
                self.launch_session(permission.run_id.clone())
                    .map_err(internal)?;
                "running"
            }
            PermissionDecision::Ask => {
                self.store
                    .update_run_status(&permission.run_id, AgentRunStatus::NeedsUserInput)
                    .await
                    .map_err(conflict)?;
                self.store
                    .append_event(AgentEvent::StateChanged {
                        run_id: permission.run_id.clone(),
                        state: "needs_user_input:permission_ask".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await
                    .map_err(internal)?;
                self.store
                    .append_audit_record(
                        &permission.project_id,
                        &permission.run_id,
                        &permission.tool,
                        permission_input_summary(updated_input.as_ref()),
                        "ask",
                        "permission requires additional user input",
                    )
                    .await;
                self.store
                    .append_conversation_item(
                        &permission.project_id,
                        Some(&permission.run_id),
                        "permission_resolved",
                        Some("system"),
                        format!("More information requested for {}", permission.tool),
                        Some(serde_json::json!({
                            "permissionId": permission.id,
                            "tool": permission.tool,
                            "toolUseId": permission.tool_use_id,
                            "decision": "ask",
                            "updatedInputProvided": updated_input.is_some(),
                        })),
                    )
                    .await;
                "needs_user_input"
            }
            PermissionDecision::Deny => {
                self.store
                    .update_run_status(&permission.run_id, AgentRunStatus::Blocked)
                    .await
                    .map_err(conflict)?;
                self.store
                    .append_event(AgentEvent::PermissionDenied {
                        run_id: permission.run_id.clone(),
                        tool: permission.tool.clone(),
                        reason: "permission denied by API".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await
                    .map_err(internal)?;
                self.store
                    .append_conversation_item(
                        &permission.project_id,
                        Some(&permission.run_id),
                        "permission_denied",
                        Some("system"),
                        format!("Permission denied for {}", permission.tool),
                        Some(serde_json::json!({
                            "permissionId": permission.id,
                            "tool": permission.tool.clone(),
                            "reason": "permission denied by API",
                        })),
                    )
                    .await;
                self.store
                    .append_audit_record(
                        &permission.project_id,
                        &permission.run_id,
                        &permission.tool,
                        "permission decision",
                        "deny",
                        "permission denied by API",
                    )
                    .await;
                "blocked"
            }
        };

        Ok(RunLifecycleOutcome {
            run_id: permission.run_id,
            status: status.to_string(),
        })
    }
}

fn permission_input_summary(updated_input: Option<&Value>) -> String {
    updated_input.map_or_else(
        || "no updatedInput".to_string(),
        |input| {
            let bytes = serde_json::to_vec(input).unwrap_or_default();
            format!("updatedInputSha256={}", crate::types::sha256_hex(&bytes))
        },
    )
}
