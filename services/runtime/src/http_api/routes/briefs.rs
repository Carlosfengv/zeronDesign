use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/briefs/{brief_id}", get(get_brief))
        .route("/briefs/{brief_id}/confirm", post(confirm_brief))
}

async fn get_brief(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(brief_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<BriefResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("briefId", &brief_id)?;
    let response = load_brief_response(&state, &brief_id).await?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &response.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    Ok(Json(response))
}

async fn confirm_brief(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<RunLifecycleService>,
    Path(brief_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<BriefResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("briefId", &brief_id)?;
    let response = load_brief_response(&state, &brief_id).await?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &response.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;

    match response.status {
        crate::types::BriefStatus::Confirmed => return Ok(Json(response)),
        crate::types::BriefStatus::Superseded => {
            return Err(conflict_error(anyhow::anyhow!(
                "brief {brief_id} is superseded"
            )))
        }
        crate::types::BriefStatus::Draft => {}
    }
    let run = state
        .store
        .get_run(&response.run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {}", response.run_id)))?;
    if run.phase != AgentPhase::Brief
        || run.status != crate::types::AgentRunStatus::NeedsUserInput
        || run.brief_version.as_deref() != Some(brief_id.as_str())
    {
        return Err(conflict_error(anyhow::anyhow!(
            "brief {brief_id} is not awaiting confirmation"
        )));
    }
    service
        .continue_run(&response.run_id, "confirm".to_string())
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(load_brief_response(&state, &brief_id).await?))
}

async fn load_brief_response(
    state: &AppState,
    brief_id: &str,
) -> Result<BriefResponse, (StatusCode, Json<ErrorResponse>)> {
    let brief = state
        .store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let status = state
        .store
        .brief_status(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief status not found: {brief_id}")))?;
    let run_id = state
        .store
        .brief_run_id(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief run not found: {brief_id}")))?;
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    Ok(BriefResponse {
        brief_id: brief_id.to_string(),
        project_id: run.project_id,
        run_id,
        status,
        run_status: run.status,
        brief,
    })
}
