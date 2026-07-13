use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs/{run_id}/cancel", post(cancel_run))
}

async fn cancel_run(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<RunLifecycleService>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let outcome = service.cancel(&run_id).await.map_err(run_lifecycle_error)?;
    Ok(Json(RunStatusResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
