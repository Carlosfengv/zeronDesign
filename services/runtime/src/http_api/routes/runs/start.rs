use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs", post(start_run))
}

async fn start_run(
    Extension(service): Extension<RunLifecycleService>,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    let outcome = service
        .start(request.into())
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(StartRunResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
