use super::super::super::*;
use super::start::is_brief_confirmation_message;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs/{run_id}/continue", post(continue_run))
}

async fn continue_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<ContinueRunRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    validate_required_string("userMessage", &request.user_message)?;
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    if run.status.is_terminal() {
        return Err(conflict_error(anyhow::anyhow!(
            "run {run_id} is already terminal with status {:?}",
            run.status
        )));
    }
    state
        .store
        .append_conversation_item(
            &run.project_id,
            Some(&run_id),
            "user_message",
            Some("user"),
            request.user_message.clone(),
            None,
        )
        .await;
    if run.phase == AgentPhase::Brief
        && run.status == AgentRunStatus::NeedsUserInput
        && run.brief_version.is_some()
        && is_brief_confirmation_message(&request.user_message)
    {
        let brief_id = run.brief_version.clone().unwrap();
        state
            .store
            .confirm_brief(&run_id, &brief_id)
            .await
            .map_err(internal_error)?;
        state
            .store
            .update_run_status(&run_id, AgentRunStatus::Completed)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::RunCompleted {
                run_id: run_id.clone(),
                status: "completed".to_string(),
                summary: "Brief confirmed.".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run_id),
                "run_completed",
                Some("system"),
                "Brief confirmed.",
                Some(serde_json::json!({ "briefId": brief_id })),
            )
            .await;
        return Ok(Json(RunStatusResponse {
            run_id,
            status: "completed".to_string(),
        }));
    }
    if run.status == AgentRunStatus::Running {
        state.store.request_continue_interrupt(&run_id).await;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.clone(),
                state: "running:continue_queued".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        return Ok(Json(RunStatusResponse {
            run_id,
            status: "running".to_string(),
        }));
    }
    if run.phase == AgentPhase::Edit {
        let design_profile_override_accepted = run.status == AgentRunStatus::NeedsUserInput
            && run.design_profile_id.is_some()
            && is_design_profile_override_message(&request.user_message)
            && has_design_profile_conflict_state(&state.store, &run_id).await;
        if design_profile_override_accepted {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run_id),
                    "design_profile_override",
                    Some("user"),
                    "DesignProfile override accepted for this run.",
                    Some(json!({
                        "designProfileId": run.design_profile_id.as_deref(),
                        "designProfileVersion": run.design_profile_version,
                        "designProfileHash": run.design_profile_hash.as_deref(),
                        "decision": "override",
                        "state": "accepted",
                        "userMessage": request.user_message.clone(),
                    })),
                )
                .await;
        }
        if let Some(conflict_reason) =
            classify_design_profile_edit_conflict(&state.store, &run, &request.user_message).await?
        {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run_id),
                    "approval_request",
                    Some("assistant"),
                    format!("DesignProfile conflict requires confirmation: {conflict_reason}"),
                    Some(json!({
                        "reason": conflict_reason,
                        "designProfileId": run.design_profile_id.as_deref(),
                        "state": "needs_user_input:design_profile_conflict",
                    })),
                )
                .await;
            state
                .store
                .update_run_status(&run_id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: run_id.clone(),
                    state: "needs_user_input:design_profile_conflict".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(Json(RunStatusResponse {
                run_id,
                status: "needs_user_input".to_string(),
            }));
        }
        match edit::classify_edit_intent(&state.store, &run, &request.user_message)
            .await
            .map_err(internal_error)?
        {
            EditIntent::Compatible => {}
            EditIntent::BriefConflict { reason } => {
                state
                    .store
                    .append_conversation_item(
                        &run.project_id,
                        Some(&run_id),
                        "approval_request",
                        Some("assistant"),
                        format!("This edit may change the confirmed Brief: {reason}"),
                        Some(serde_json::json!({ "reason": reason })),
                    )
                    .await;
                state
                    .store
                    .update_run_status(&run_id, AgentRunStatus::NeedsUserInput)
                    .await
                    .map_err(conflict_error)?;
                state
                    .store
                    .append_event(AgentEvent::StateChanged {
                        run_id: run_id.clone(),
                        state: "needs_user_input:brief_conflict".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await
                    .map_err(internal_error)?;
                return Ok(Json(RunStatusResponse {
                    run_id,
                    status: "needs_user_input".to_string(),
                }));
            }
        }
    }
    state
        .store
        .update_run_status(&run_id, AgentRunStatus::Running)
        .await
        .map_err(conflict_error)?;
    state
        .store
        .append_event(AgentEvent::StateChanged {
            run_id: run_id.clone(),
            state: "running".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .map_err(internal_error)?;
    spawn_session(state, run_id.clone());
    Ok(Json(RunStatusResponse {
        run_id,
        status: "running".to_string(),
    }))
}
