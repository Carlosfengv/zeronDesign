use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/conversation",
            get(project_conversation),
        )
        .route(
            "/projects/{project_id}/runtime-state",
            get(project_runtime_state),
        )
}

async fn project_conversation(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ConversationQuery>,
) -> Result<Json<ConversationListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let mut items = state.store.conversation_items(&project_id).await;
    if !query.include_debug {
        items.retain(|item| item.visibility == "user");
    }
    Ok(Json(ConversationListResponse { project_id, items }))
}

async fn project_runtime_state(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProjectRuntimeStateResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let current = state
        .store
        .current_project_version(&project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current version not found for project: {project_id}"
            ))
        })?;
    let binding = state
        .store
        .current_project_sandbox_binding(&project_id)
        .await
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "editable sandbox binding not found for project: {project_id}"
            ))
        })?;
    let source_snapshot_uri = current.source_snapshot_uri.clone().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "source snapshot not found for current version: {}",
            current.id
        ))
    })?;
    let current_run = state.store.get_run(&current.created_by_run_id).await;
    let template_key = if let Some(run) = current_run.as_ref() {
        if let Some(brief_id) = &run.brief_version {
            state
                .store
                .get_brief(brief_id)
                .await
                .map(|brief| brief.recommended_template)
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };
    let style_contract = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/style-contract.json",
    )
    .await;
    let latest_build = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "outputs/build/latest.json",
    )
    .await;
    let dependency_state = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/dependency-state.json",
    )
    .await;
    let preview = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/preview.json",
    )
    .await;

    Ok(Json(ProjectRuntimeStateResponse {
        project_id,
        current_version_id: current.id,
        sandbox_binding_id: binding.id,
        source_snapshot_uri,
        app_root: "project".to_string(),
        template_key,
        style_contract_path: style_contract
            .as_ref()
            .map(|_| "/workspace/state/style-contract.json".to_string()),
        style_contract,
        latest_build,
        dependency_state,
        preview,
    }))
}
