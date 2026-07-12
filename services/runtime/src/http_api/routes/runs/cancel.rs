use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs/{run_id}/cancel", post(cancel_run))
}

async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    let cancelled = state
        .store
        .update_run_status(&run_id, AgentRunStatus::Cancelled)
        .await
        .map_err(run_update_error)?;
    if let Some(run) = state.store.get_run(&run_id).await {
        let workspace_root = effective_workspace_root(&state.config, &run.project_id);
        cancel_run_sandbox_resources(&state.config, &state.store, &run, workspace_root.clone())
            .await
            .map_err(internal_error)?;
        if state.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes
            && run.sandbox_id.is_some()
        {
            let mut ctx = ToolContext::new(state.store.clone(), run, workspace_root);
            ctx.remote_workspace = true;
            ctx.runtime_storage_dir = state.config.runtime_storage_dir.clone();
            let backend = SandboxChannelWorkspaceBackend::from_runtime_config(&state.config)
                .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
            cleanup_staged_writes_for_run_backend(&backend, &ctx, &run_id)
                .await
                .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
        } else {
            cleanup_staged_writes_for_run(&workspace_root, &run_id);
        }
    }
    state
        .store
        .append_event(AgentEvent::RunCompleted {
            run_id: run_id.clone(),
            status: "cancelled".to_string(),
            summary: "Run cancelled.".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .map_err(internal_error)?;
    Ok(Json(RunStatusResponse {
        run_id: cancelled.id,
        status: "cancelled".to_string(),
    }))
}
