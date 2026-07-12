use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs/{run_id}/continue", post(continue_run))
}

async fn continue_run(
    Extension(service): Extension<RunLifecycleService>,
    Path(run_id): Path<String>,
    Json(request): Json<ContinueRunRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    validate_required_string("userMessage", &request.user_message)?;
    let outcome = service
        .continue_run(&run_id, request.user_message)
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(RunStatusResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
