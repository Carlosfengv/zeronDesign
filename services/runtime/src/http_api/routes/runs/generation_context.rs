use super::super::super::*;
use crate::generation_context::{status_for_run, GenerationContextStatus};

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/runs/{run_id}/generation-context-status",
        get(get_generation_context_status),
    )
}

async fn get_generation_context_status(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<GenerationContextStatus>, (StatusCode, Json<ErrorResponse>)> {
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
        PROJECT_READ_OPERATION,
    )
    .await?;
    Ok(Json(status_for_run(&run)))
}
