use super::super::super::*;
use crate::run_metrics::{calculate_run_efficiency_metrics, RunEfficiencyMetrics};

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/runs/{run_id}/efficiency-metrics",
        get(get_run_efficiency_metrics),
    )
}

async fn get_run_efficiency_metrics(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunEfficiencyMetrics>, (StatusCode, Json<ErrorResponse>)> {
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
    let events = state.store.events(&run_id).await;
    Ok(Json(calculate_run_efficiency_metrics(&run, &events)))
}
