use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs", post(start_run))
}

async fn start_run(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<RunLifecycleService>,
    headers: HeaderMap,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &request.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let outcome = service
        .start(request.into())
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(StartRunResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
