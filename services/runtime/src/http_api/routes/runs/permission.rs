use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route(
        "/permissions/{permission_id}/decision",
        post(resolve_permission),
    )
}

async fn resolve_permission(
    State(state): State<AppState>,
    Path(permission_id): Path<String>,
    Json(request): Json<ResolvePermissionRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("permissionId", &permission_id)?;
    let pending_permission = state
        .store
        .pending_permission(&permission_id)
        .await
        .ok_or_else(|| not_found(format!("permission request not found: {permission_id}")))?;
    if pending_permission.status != "pending" {
        return Err(conflict_error(anyhow::anyhow!(
            "permission request {permission_id} is already {}",
            pending_permission.status
        )));
    }
    let permission_run = state
        .store
        .get_run(&pending_permission.run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {}", pending_permission.run_id)))?;
    if permission_run.status.is_terminal() {
        return Err(conflict_error(anyhow::anyhow!(
            "run {} is already terminal with status {:?}",
            permission_run.id,
            permission_run.status
        )));
    }
    let permission = state
        .store
        .resolve_permission(&permission_id, request.decision.as_str())
        .await
        .map_err(internal_error)?;
    let status = match request.decision {
        PermissionDecision::Allow => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::Running)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: permission.run_id.clone(),
                    state: "running".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            state
                .store
                .append_audit_record(
                    &permission.project_id,
                    &permission.run_id,
                    &permission.tool,
                    request
                        .updated_input
                        .as_ref()
                        .map(|_| "updatedInput provided")
                        .unwrap_or("no updatedInput"),
                    "allow",
                    "permission resolved by API",
                )
                .await;
            spawn_session(state, permission.run_id.clone());
            "running"
        }
        PermissionDecision::Ask => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: permission.run_id.clone(),
                    state: "needs_user_input:permission_ask".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            state
                .store
                .append_audit_record(
                    &permission.project_id,
                    &permission.run_id,
                    &permission.tool,
                    request
                        .updated_input
                        .as_ref()
                        .map(|_| "updatedInput provided")
                        .unwrap_or("permission decision"),
                    "ask",
                    "permission requires additional user input",
                )
                .await;
            "needs_user_input"
        }
        PermissionDecision::Deny => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::Blocked)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::PermissionDenied {
                    run_id: permission.run_id.clone(),
                    tool: permission.tool.clone(),
                    reason: "permission denied by API".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            state
                .store
                .append_conversation_item(
                    &permission.project_id,
                    Some(&permission.run_id),
                    "permission_denied",
                    Some("system"),
                    format!("Permission denied for {}", permission.tool),
                    Some(serde_json::json!({
                        "tool": permission.tool.clone(),
                        "reason": "permission denied by API",
                    })),
                )
                .await;
            state
                .store
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
    Ok(Json(RunStatusResponse {
        run_id: permission.run_id,
        status: status.to_string(),
    }))
}
